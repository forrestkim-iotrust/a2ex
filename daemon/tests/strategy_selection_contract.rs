mod support;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use a2ex_onboarding::{
    ApplyStrategySelectionOverride, ApplyStrategySelectionOverrideRequest,
    ApproveStrategySelectionRequest, GuidedOnboardingInspectionRequest,
    InspectStrategySelectionRequest, StrategySelectionApprovalInput, StrategySelectionError,
    StrategySelectionSensitivityClass, StrategySelectionStatus, apply_strategy_selection_override,
    approve_strategy_selection, inspect_guided_onboarding, inspect_strategy_selection,
};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams, ResourceContents},
    transport::TokioChildProcess,
};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value, json};
use support::skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle};
use tempfile::tempdir;

const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";

const STRATEGY_SELECTIONS_TABLE: &str = "onboarding_strategy_selections";
const STRATEGY_SELECTION_OVERRIDES_TABLE: &str = "onboarding_strategy_selection_overrides";
const STRATEGY_SELECTION_STREAM_TYPE: &str = "strategy_selection";
const STRATEGY_SELECTION_MATERIALIZED_EVENT: &str = "strategy_selection_materialized";

const READY_PATH_ENTRY_SKILL_MD: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.12
compatible_daemon: ">=0.1.0"
name: Prediction Spread Arb
summary: Capture spread dislocations between prediction venues.
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
    revision: 2026.03.10
---
# Overview

Track spread divergences after setup is complete.

# Owner Decisions

- Approve max spread budget.
"#;

const READY_PATH_OWNER_SETUP_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

#[tokio::test]
async fn strategy_selection_contract_requires_canonical_persistence_snapshot_truth_and_materialize_journal()
 {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let bootstrap = bootstrap_install_live(&client, &entry_url, workspace_root.path()).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();
    apply_action(
        &client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let ready = apply_action(
        &client,
        &install_id,
        json!({
            "kind": "resolve_owner_decision",
            "step_key": "approve-max-spread-budget",
            "resolution": "approved"
        }),
    )
    .await;
    assert_eq!(ready["aggregate_status"], "ready");

    let inspection = inspect_guided_onboarding(GuidedOnboardingInspectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
    })
    .await
    .expect("ready install should remain inspectable through the direct onboarding seam");
    let handoff = inspection
        .proposal_handoff
        .expect("ready onboarding must expose a proposal handoff before strategy selection");

    let load = call_tool_json(
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(handoff.entry_url.to_string()))]),
    )
    .await;
    let proposal_id = load["session_id"]
        .as_str()
        .expect("session id string")
        .to_owned();
    let proposal = call_tool_json(
        &client,
        TOOL_GENERATE_PROPOSAL_PACKET,
        json_map([("session_id", Value::String(proposal_id.clone()))]),
    )
    .await;
    assert_eq!(proposal["proposal_readiness"], "ready");
    let proposal_uri = proposal["proposal_uri"]
        .as_str()
        .expect("proposal uri string")
        .to_owned();
    let proposal_resource = read_resource_json(&client, proposal_uri.clone()).await;

    let connection = Connection::open(state_db_path(workspace_root.path()))
        .expect("state db should remain inspectable after proposal generation");
    let mut gaps = Vec::new();

    let tables = table_names(&connection);
    if !tables.contains(STRATEGY_SELECTIONS_TABLE) {
        gaps.push(format!(
            "missing canonical table {STRATEGY_SELECTIONS_TABLE} for install/proposal-keyed strategy selections"
        ));
    }
    if !tables.contains(STRATEGY_SELECTION_OVERRIDES_TABLE) {
        gaps.push(format!(
            "missing canonical table {STRATEGY_SELECTION_OVERRIDES_TABLE} for typed override history"
        ));
    }

    let required_selection_columns = [
        "install_id",
        "proposal_id",
        "selection_id",
        "status",
        "proposal_revision",
        "proposal_snapshot_json",
        "recommendation_basis_json",
        "readiness_sensitivity_summary_json",
        "approval_json",
        "created_at",
        "updated_at",
    ];
    let required_override_columns = [
        "install_id",
        "proposal_id",
        "selection_id",
        "selection_revision",
        "override_key",
        "previous_value_json",
        "new_value_json",
        "rationale",
        "provenance_json",
        "sensitivity_class",
        "created_at",
    ];

    if tables.contains(STRATEGY_SELECTIONS_TABLE) {
        assert_required_columns(
            &connection,
            STRATEGY_SELECTIONS_TABLE,
            &required_selection_columns,
            &mut gaps,
        );

        if has_required_columns(
            &connection,
            STRATEGY_SELECTIONS_TABLE,
            &required_selection_columns,
        ) {
            let row = connection
                .query_row(
                    "SELECT selection_id, status, proposal_revision, proposal_snapshot_json,
                            recommendation_basis_json, readiness_sensitivity_summary_json,
                            approval_json
                     FROM onboarding_strategy_selections
                     WHERE install_id = ?1 AND proposal_id = ?2",
                    [install_id.as_str(), proposal_id.as_str()],
                    |record| {
                        Ok((
                            record.get::<_, String>(0)?,
                            record.get::<_, String>(1)?,
                            record.get::<_, i64>(2)?,
                            record.get::<_, String>(3)?,
                            record.get::<_, String>(4)?,
                            record.get::<_, String>(5)?,
                            record.get::<_, String>(6)?,
                        ))
                    },
                )
                .optional()
                .expect("selection row query should execute once the table exists");

            match row {
                Some((selection_id, status, proposal_revision, snapshot_json, basis_json, sensitivity_json, approval_json)) => {
                    if status != "recommended" {
                        gaps.push(format!(
                            "canonical selection row should start in status=recommended after proposal handoff, got {status}"
                        ));
                    }
                    if proposal_revision != proposal_resource["proposal_revision"].as_i64().unwrap_or_default() {
                        gaps.push("canonical strategy selection row must persist the proposal revision it snapshots".to_owned());
                    }

                    let snapshot: Value = serde_json::from_str(&snapshot_json)
                        .expect("proposal_snapshot_json should stay valid JSON");
                    if snapshot["proposal_readiness"] != proposal_resource["proposal"]["proposal_readiness"] {
                        gaps.push("strategy selection snapshot must preserve proposal_readiness from the proposal packet instead of recomputing it later".to_owned());
                    }
                    if snapshot["plan_summary"]["bundle_id"]
                        != proposal_resource["proposal"]["plan_summary"]["bundle_id"]
                    {
                        gaps.push("strategy selection snapshot must preserve the proposal plan summary bundle_id from the proposal packet".to_owned());
                    }
                    if snapshot["capital_profile"]["completeness"]
                        != proposal_resource["proposal"]["capital_profile"]["completeness"]
                    {
                        gaps.push("strategy selection snapshot must preserve the truthful capital completeness from the proposal packet".to_owned());
                    }
                    if snapshot["cost_profile"]["completeness"]
                        != proposal_resource["proposal"]["cost_profile"]["completeness"]
                    {
                        gaps.push("strategy selection snapshot must preserve the truthful cost completeness from the proposal packet".to_owned());
                    }
                    if !snapshot["owner_override_points"]
                        .as_array()
                        .is_some_and(|items| items.iter().any(|item| item["decision_key"] == "approve-max-spread-budget"))
                    {
                        gaps.push("strategy selection snapshot must preserve proposal owner_override_points for later typed overrides".to_owned());
                    }

                    let basis: Value = serde_json::from_str(&basis_json)
                        .expect("recommendation_basis_json should stay valid JSON");
                    if basis["source_kind"] != "proposal_packet" {
                        gaps.push("recommendation_basis_json must record source_kind=proposal_packet so the selection truth is anchored to the real proposal seam".to_owned());
                    }
                    if basis["proposal_uri"] != proposal_uri {
                        gaps.push("recommendation_basis_json must retain proposal_uri provenance for the exact packet that was materialized".to_owned());
                    }
                    if basis["proposal_revision"] != proposal_resource["proposal_revision"] {
                        gaps.push("recommendation_basis_json must retain the proposal revision that was materialized".to_owned());
                    }

                    let sensitivity: Value = serde_json::from_str(&sensitivity_json)
                        .expect("readiness_sensitivity_summary_json should stay valid JSON");
                    if !sensitivity["readiness_sensitive_override_keys"]
                        .as_array()
                        .is_some_and(|keys| keys.iter().any(|key| key == "approve-max-spread-budget"))
                    {
                        gaps.push("readiness_sensitivity_summary_json must classify approve-max-spread-budget as readiness_sensitive so later slices can trigger reevaluation deterministically".to_owned());
                    }
                    if sensitivity.get("advisory_override_keys").and_then(Value::as_array).is_none() {
                        gaps.push("readiness_sensitivity_summary_json must expose advisory_override_keys even when empty".to_owned());
                    }

                    let approval: Value = serde_json::from_str(&approval_json)
                        .expect("approval_json should stay valid JSON");
                    if approval.get("status").and_then(Value::as_str) != Some("pending") {
                        gaps.push("approval_json must expose status=pending before approval so approval provenance stays inspectable instead of implied".to_owned());
                    }
                    if approval.get("approved_revision").is_none() {
                        gaps.push("approval_json must carry approved_revision even before approval so stale-approval checks have a stable canonical slot".to_owned());
                    }

                    let override_count: i64 = connection
                        .query_row(
                            "SELECT COUNT(*) FROM onboarding_strategy_selection_overrides
                             WHERE install_id = ?1 AND proposal_id = ?2 AND selection_id = ?3",
                            [install_id.as_str(), proposal_id.as_str(), selection_id.as_str()],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    if override_count != 0 {
                        gaps.push(format!(
                            "freshly materialized selections should start with zero overrides, got {override_count}"
                        ));
                    }
                }
                None => gaps.push(
                    "ready install → proposal handoff must auto-materialize one canonical strategy selection row keyed by install_id + proposal_id".to_owned(),
                ),
            }
        }
    }

    if tables.contains(STRATEGY_SELECTION_OVERRIDES_TABLE) {
        assert_required_columns(
            &connection,
            STRATEGY_SELECTION_OVERRIDES_TABLE,
            &required_override_columns,
            &mut gaps,
        );
    }

    let materialized_event: Option<String> = connection
        .query_row(
            "SELECT payload_json
             FROM event_journal
             WHERE stream_type = ?1 AND stream_id = ?2 AND event_type = ?3
             ORDER BY created_at DESC, event_id DESC
             LIMIT 1",
            [
                STRATEGY_SELECTION_STREAM_TYPE,
                proposal_id.as_str(),
                STRATEGY_SELECTION_MATERIALIZED_EVENT,
            ],
            |row| row.get(0),
        )
        .optional()
        .expect("event journal query should remain inspectable");

    match materialized_event {
        Some(payload_json) => {
            let payload: Value = serde_json::from_str(&payload_json)
                .expect("strategy-selection materialized journal payload should be JSON");
            if payload["install_id"] != install_id {
                gaps.push("strategy_selection_materialized journal payload must retain install_id provenance".to_owned());
            }
            if payload["proposal_id"] != proposal_id {
                gaps.push("strategy_selection_materialized journal payload must retain proposal_id provenance".to_owned());
            }
            if payload["proposal_revision"] != proposal_resource["proposal_revision"] {
                gaps.push("strategy_selection_materialized journal payload must retain the proposal revision that was materialized".to_owned());
            }
        }
        None => gaps.push(
            "event_journal must record strategy_selection_materialized when the canonical recommendation is created".to_owned(),
        ),
    }

    let direct_inspection = inspect_strategy_selection(InspectStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
    })
    .await
    .expect("direct strategy-selection inspection should reread the canonical summary after materialization");
    if direct_inspection.summary.status != StrategySelectionStatus::Recommended {
        gaps.push("direct strategy-selection inspection must report status=recommended immediately after materialize".to_owned());
    }
    if direct_inspection.summary.created_at != direct_inspection.summary.updated_at {
        gaps.push("freshly materialized direct strategy-selection summaries must start with created_at == updated_at".to_owned());
    }
    if direct_inspection.summary.approval.approved_by.is_some()
        || direct_inspection.summary.approval.approved_at.is_some()
        || direct_inspection.summary.approval.note.is_some()
    {
        gaps.push("direct strategy-selection inspection must keep approval provenance empty while status=pending".to_owned());
    }

    let inspection_after_override =
        apply_strategy_selection_override(ApplyStrategySelectionOverrideRequest {
            state_db_path: state_db_path(workspace_root.path()),
            install_id: install_id.clone(),
            proposal_id: proposal_id.clone(),
            selection_id: direct_inspection.summary.selection_id.clone(),
            override_record: ApplyStrategySelectionOverride {
                key: "approve-max-spread-budget".to_owned(),
                value: json!({ "resolution": "approved" }),
                rationale: "owner approved the max spread budget after reviewing the proposal"
                    .to_owned(),
                provenance: None,
            },
        })
        .await
        .expect("direct override application should persist typed canonical override history");
    if inspection_after_override.summary.status != StrategySelectionStatus::Recommended {
        gaps.push(
            "applying a direct override must not implicitly approve the strategy selection"
                .to_owned(),
        );
    }
    if inspection_after_override.summary.selection_revision != 2 {
        gaps.push(format!(
            "first direct override must advance selection_revision to 2, got {}",
            inspection_after_override.summary.selection_revision
        ));
    }
    if inspection_after_override.summary.updated_at < inspection_after_override.summary.created_at {
        gaps.push(
            "applying a direct override must not regress updated_at behind the materialized timestamp"
                .to_owned(),
        );
    }
    match inspection_after_override.overrides.as_slice() {
        [override_record] => {
            if override_record.selection_revision != inspection_after_override.summary.selection_revision {
                gaps.push("direct override history must record the post-mutation selection_revision that produced it".to_owned());
            }
            if override_record.created_at != inspection_after_override.summary.updated_at {
                gaps.push("direct override history timestamp must match the summary updated_at for the same mutation".to_owned());
            }
            if override_record.sensitivity_class
                != StrategySelectionSensitivityClass::ReadinessSensitive
            {
                gaps.push("direct override history must preserve readiness_sensitive classification for approve-max-spread-budget".to_owned());
            }
            if override_record.provenance != json!({ "source": "owner_override" }) {
                gaps.push("direct override history must persist the default owner_override provenance when callers omit it".to_owned());
            }
        }
        overrides => gaps.push(format!(
            "direct strategy-selection inspection must expose exactly one override after the first mutation, got {}",
            overrides.len()
        )),
    }

    let approved = approve_strategy_selection(ApproveStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        selection_id: inspection_after_override.summary.selection_id.clone(),
        expected_selection_revision: inspection_after_override.summary.selection_revision,
        approval: StrategySelectionApprovalInput {
            approved_by: "owner".to_owned(),
            note: Some(
                "approved after rereading the canonical snapshot and override history".to_owned(),
            ),
        },
    })
    .await
    .expect("direct approval should persist canonical approval provenance");
    if approved.status != StrategySelectionStatus::Approved {
        gaps.push("direct approval must persist status=approved".to_owned());
    }
    if approved.approval.approved_revision != Some(approved.selection_revision) {
        gaps.push("direct approval must preserve the exact approved selection revision".to_owned());
    }
    if approved.approval.approved_by.as_deref() != Some("owner") {
        gaps.push("direct approval must preserve approved_by provenance".to_owned());
    }
    if approved.approval.approved_at.as_deref() != Some(approved.updated_at.as_str()) {
        gaps.push("direct approval must stamp approved_at with the same canonical updated_at used by the approved summary".to_owned());
    }
    if approved.updated_at < inspection_after_override.summary.updated_at {
        gaps.push(
            "direct approval must not regress updated_at behind the override timestamp".to_owned(),
        );
    }

    let reread_after_approval = inspect_strategy_selection(InspectStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
    })
    .await
    .expect("direct strategy-selection reread should preserve approval provenance after approval");
    if reread_after_approval.summary != approved {
        gaps.push("direct strategy-selection reread must match the approved canonical summary instead of replaying stale pre-approval state".to_owned());
    }

    let stale_approval_error = approve_strategy_selection(ApproveStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        selection_id: approved.selection_id.clone(),
        expected_selection_revision: 1,
        approval: StrategySelectionApprovalInput {
            approved_by: "owner".to_owned(),
            note: Some("exercise direct stale revision diagnostics".to_owned()),
        },
    })
    .await
    .expect_err("direct stale approval attempts must stay typed");
    if !matches!(
        stale_approval_error,
        StrategySelectionError::StaleSelectionRevision {
            expected_selection_revision: 1,
            actual_selection_revision
        } if actual_selection_revision == approved.selection_revision
    ) {
        gaps.push(format!(
            "direct stale approval attempts must return StrategySelectionError::StaleSelectionRevision with the current revision, got {stale_approval_error:?}"
        ));
    }

    let reopened_inspection = inspect_guided_onboarding(GuidedOnboardingInspectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
    })
    .await
    .expect("ready install should stay inspectable after reconnect-style direct reread");
    if reopened_inspection.proposal_handoff.is_none() {
        gaps.push("direct reconnect-safe inspection must preserve proposal_handoff after strategy selection materialization".to_owned());
    }

    assert!(
        gaps.is_empty(),
        "S01 direct strategy-selection contract missing canonical state or truthful snapshotting: {}",
        gaps.join("; ")
    );

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly");
}

async fn ready_path_harness() -> SkillBundleHarness {
    spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(READY_PATH_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(READY_PATH_OWNER_SETUP_MD),
        ),
    ])
    .await
}

fn state_db_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".a2ex-daemon/state.db")
}

fn table_names(connection: &Connection) -> HashSet<String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .expect("sqlite_master query should prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("sqlite_master query should run");
    rows.collect::<Result<HashSet<_>, _>>()
        .expect("table names should collect")
}

fn table_columns(connection: &Connection, table: &str) -> HashSet<String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("table_info pragma should prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("table_info pragma should run");
    rows.collect::<Result<HashSet<_>, _>>()
        .expect("table columns should collect")
}

fn has_required_columns(connection: &Connection, table: &str, required: &[&str]) -> bool {
    let columns = table_columns(connection, table);
    required.iter().all(|column| columns.contains(*column))
}

fn assert_required_columns(
    connection: &Connection,
    table: &str,
    required: &[&str],
    gaps: &mut Vec<String>,
) {
    let columns = table_columns(connection, table);
    for column in required {
        if !columns.contains(*column) {
            gaps.push(format!("table {table} missing required column {column}"));
        }
    }
}

async fn spawn_live_client() -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root canonicalizes");
    let manifest_path = workspace_root.join("Cargo.toml");

    let mut command = tokio::process::Command::new(cargo);
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("-p")
        .arg("a2ex-mcp")
        .arg("--bin")
        .arg("a2ex-mcp")
        .current_dir(&workspace_root);

    let transport = TokioChildProcess::new(command).expect("live stdio server should spawn");
    ().serve(transport)
        .await
        .expect("rmcp client should initialize against the live stdio server")
}

async fn bootstrap_install_live(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_url: &str,
    workspace_root: &Path,
) -> Value {
    call_tool_json(
        client,
        TOOL_BOOTSTRAP_INSTALL,
        json_map([
            ("install_url", Value::String(install_url.to_owned())),
            (
                "workspace_root",
                Value::String(workspace_root.display().to_string()),
            ),
        ]),
    )
    .await
}

async fn apply_action(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_id: &str,
    action: Value,
) -> Value {
    call_tool_json(
        client,
        TOOL_APPLY_ONBOARDING_ACTION,
        json_map([
            ("install_id", Value::String(install_id.to_owned())),
            ("action", action),
        ]),
    )
    .await
}

async fn call_tool_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool_name: &str,
    arguments: Map<String, Value>,
) -> Value {
    client
        .call_tool(CallToolRequestParams::new(tool_name.to_owned()).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("tool {tool_name} should succeed: {error}"))
        .structured_content
        .expect("tool result should include structured content")
}

async fn read_resource_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Value {
    let response = client
        .read_resource(ReadResourceRequestParams::new(uri.clone()))
        .await
        .unwrap_or_else(|error| panic!("resource {uri} should be readable: {error}"));
    assert_eq!(response.contents.len(), 1);
    let text = match &response.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text,
        other => panic!("expected text resource contents, got {other:?}"),
    };
    serde_json::from_str(text).expect("resource contents should be valid JSON")
}

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}
