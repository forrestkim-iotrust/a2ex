mod support;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_onboarding::{
    ApplyStrategySelectionOverride, ApplyStrategySelectionOverrideRequest,
    ApproveStrategySelectionRequest, GuidedOnboardingInspectionRequest,
    InspectStrategySelectionRequest, RouteReadinessAction, RouteReadinessActionRequest,
    RouteReadinessEvaluationRequest, RouteReadinessInspectionRequest, RouteReadinessStaleStatus,
    StrategySelectionApprovalInput, StrategySelectionStatus, apply_route_readiness_action,
    apply_strategy_selection_override, approve_strategy_selection, evaluate_route_readiness,
    inspect_guided_onboarding, inspect_route_readiness, inspect_strategy_selection,
};
use a2ex_policy::BaselinePolicy;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
};
use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ResourceContents},
    transport::TokioChildProcess,
};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value, json};
use support::{
    across_harness::FakeAcrossTransport,
    hyperliquid_harness::FakeHyperliquidTransport,
    prediction_market_harness::FakePredictionMarketTransport,
    skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle},
};
use tempfile::tempdir;

const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";

const APPROVED_RUNTIME_HANDOFFS_TABLE: &str = "onboarding_strategy_runtime_handoffs";
const STRATEGY_RUNTIME_STREAM_TYPE: &str = "strategy_runtime_handoff";
const STRATEGY_RUNTIME_HANDOFF_EVENT: &str = "strategy_runtime_handoff_persisted";
const STRATEGY_RUNTIME_ELIGIBILITY_EVENT: &str = "strategy_runtime_eligibility_changed";

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

#[derive(Default, Clone)]
struct RecordingSigner {
    handoffs: Arc<Mutex<Vec<String>>>,
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, request: &ExecutionRequest) {
        self.handoffs
            .lock()
            .expect("handoff lock")
            .push(request.action_kind.clone());
    }
}

struct ApprovingBridge;

#[async_trait]
impl SignerBridge for ApprovingBridge {
    async fn request_approval(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalResult, a2ex_signer_bridge::SignerBridgeError> {
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(request),
        })
    }
}

#[tokio::test]
async fn strategy_runtime_handoff_contract_requires_canonical_handoff_readiness_invalidation_and_idempotent_approval()
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
        .expect("ready onboarding must expose a proposal handoff before strategy approval");

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

    let selection_before_approval = inspect_strategy_selection(InspectStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
    })
    .await
    .expect("proposal generation should materialize a direct strategy selection record");
    assert_eq!(
        selection_before_approval.summary.status,
        StrategySelectionStatus::Recommended
    );

    let routing_service = routed_daemon_service(workspace_root.path()).await;
    let submit = routing_service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request("req-runtime-handoff-1")
        .await
        .expect("preview builds");
    let route_id = expected_route_id(&preview);

    let first_eval = evaluate_route_readiness(RouteReadinessEvaluationRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        request_id: "req-runtime-handoff-1".to_owned(),
    })
    .await
    .expect("S02 direct contract should start from the real route-readiness seam");
    assert_eq!(first_eval.identity.route_id, route_id);
    assert_eq!(first_eval.identity.request_id, "req-runtime-handoff-1");

    let reservations =
        SqliteReservationManager::open(workspace_root.path().join(".a2ex-daemon/state.db"))
            .await
            .expect("reservations open");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-runtime-handoff-1".to_owned(),
            execution_id: "req-runtime-handoff-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("reservation evidence persists");

    let second_eval = evaluate_route_readiness(RouteReadinessEvaluationRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        request_id: "req-runtime-handoff-1".to_owned(),
    })
    .await
    .expect("reservation-backed reevaluation should remain direct-inspectable");
    let approval_step =
        if second_eval.current_step_key.as_deref() == Some("satisfy_route_approvals") {
            second_eval
        } else {
            apply_route_readiness_action(RouteReadinessActionRequest {
                state_db_path: state_db_path(workspace_root.path()),
                install_id: install_id.clone(),
                proposal_id: proposal_id.clone(),
                route_id: route_id.clone(),
                action: RouteReadinessAction::CompleteStep {
                    step_key: "fund_route_capital".to_owned(),
                },
            })
            .await
            .expect("fund_route_capital should be completable once reservation evidence exists");
            inspect_route_readiness(RouteReadinessInspectionRequest {
                state_db_path: state_db_path(workspace_root.path()),
                install_id: install_id.clone(),
                proposal_id: proposal_id.clone(),
                route_id: route_id.clone(),
            })
            .await
            .expect("route readiness remains inspectable after capital completion")
        };
    assert_eq!(
        approval_step.current_step_key.as_deref(),
        Some("satisfy_route_approvals")
    );

    let ready_route = apply_route_readiness_action(RouteReadinessActionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        action: RouteReadinessAction::CompleteStep {
            step_key: "satisfy_route_approvals".to_owned(),
        },
    })
    .await
    .expect("satisfy_route_approvals should finish the canonical ready route");
    assert_eq!(ready_route.status.as_str(), "ready");
    assert!(ready_route.current_step_key.is_none());

    let approved = approve_strategy_selection(ApproveStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        selection_id: selection_before_approval.summary.selection_id.clone(),
        expected_selection_revision: selection_before_approval.summary.selection_revision,
        approval: StrategySelectionApprovalInput {
            approved_by: "owner".to_owned(),
            note: Some("approve the canonical selection after the route is ready".to_owned()),
        },
    })
    .await
    .expect("direct approval should still work while S02 handoff behavior is missing");
    assert_eq!(approved.status, StrategySelectionStatus::Approved);

    let reread_after_approval = inspect_strategy_selection(InspectStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
    })
    .await
    .expect("approved selection should remain inspectable");
    assert_eq!(reread_after_approval.summary, approved);

    let repeated_approval = approve_strategy_selection(ApproveStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        selection_id: approved.selection_id.clone(),
        expected_selection_revision: approved.selection_revision,
        approval: StrategySelectionApprovalInput {
            approved_by: "owner".to_owned(),
            note: Some("repeat the same approval to verify idempotence".to_owned()),
        },
    })
    .await
    .expect(
        "repeating approval for the same revision should stay type-safe even before S02 exists",
    );

    let overridden = apply_strategy_selection_override(ApplyStrategySelectionOverrideRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        selection_id: approved.selection_id.clone(),
        override_record: ApplyStrategySelectionOverride {
            key: "approve-max-spread-budget".to_owned(),
            value: json!({ "resolution": "approved", "budget_bps": 25 }),
            rationale: "change a readiness-sensitive approval assumption after direct approval"
                .to_owned(),
            provenance: None,
        },
    })
    .await
    .expect("readiness-sensitive overrides should remain directly inspectable after approval");
    let readiness_after_override = inspect_route_readiness(RouteReadinessInspectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
    })
    .await
    .expect("route readiness should still be directly inspectable after a post-approval override");

    let connection = Connection::open(state_db_path(workspace_root.path()))
        .expect("state db should remain inspectable after direct handoff flow");
    let tables = table_names(&connection);
    let mut gaps = Vec::new();

    if repeated_approval.updated_at != approved.updated_at
        || repeated_approval.approval.approved_at != approved.approval.approved_at
        || repeated_approval.approval.note != approved.approval.note
    {
        gaps.push(
            "repeating approval for the exact same selection revision must be idempotent instead of rewriting approval timestamps or notes"
                .to_owned(),
        );
    }

    if readiness_after_override
        .stale
        .as_ref()
        .map(|stale| stale.status)
        != Some(RouteReadinessStaleStatus::Stale)
    {
        gaps.push(
            "a readiness-sensitive override after approval must invalidate route readiness so autonomy cannot remain eligible on a stale fingerprint"
                .to_owned(),
        );
    }

    if tables.contains(APPROVED_RUNTIME_HANDOFFS_TABLE) {
        let required_columns = [
            "install_id",
            "proposal_id",
            "selection_id",
            "approved_selection_revision",
            "route_id",
            "request_id",
            "route_readiness_fingerprint",
            "route_readiness_status",
            "route_readiness_evaluated_at",
            "eligibility_status",
            "hold_reason",
            "runtime_control_mode",
            "strategy_id",
            "created_at",
            "updated_at",
        ];
        assert_required_columns(
            &connection,
            APPROVED_RUNTIME_HANDOFFS_TABLE,
            &required_columns,
            &mut gaps,
        );

        if has_required_columns(
            &connection,
            APPROVED_RUNTIME_HANDOFFS_TABLE,
            &required_columns,
        ) {
            let row = connection
                .query_row(
                    "SELECT approved_selection_revision, route_id, request_id,
                            route_readiness_fingerprint, route_readiness_status,
                            route_readiness_evaluated_at, eligibility_status,
                            hold_reason, runtime_control_mode, strategy_id
                     FROM onboarding_strategy_runtime_handoffs
                     WHERE install_id = ?1 AND proposal_id = ?2 AND selection_id = ?3",
                    [
                        install_id.as_str(),
                        proposal_id.as_str(),
                        approved.selection_id.as_str(),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, Option<String>>(9)?,
                        ))
                    },
                )
                .optional()
                .expect("handoff row query should execute once schema exists");

            match row {
                Some((approved_revision, stored_route_id, stored_request_id, fingerprint, readiness_status, evaluated_at, eligibility_status, hold_reason, runtime_control_mode, strategy_id)) => {
                    if approved_revision != i64::from(approved.selection_revision) {
                        gaps.push("canonical handoff row must persist the approved selection revision".to_owned());
                    }
                    if stored_route_id != route_id {
                        gaps.push("canonical handoff row must link approval to the ready route_id".to_owned());
                    }
                    if stored_request_id != first_eval.identity.request_id {
                        gaps.push("canonical handoff row must link approval to the route request_id that produced readiness".to_owned());
                    }
                    if fingerprint.is_empty() {
                        gaps.push("canonical handoff row must persist the readiness fingerprint it relied on".to_owned());
                    }
                    if readiness_status != "ready" {
                        gaps.push(format!(
                            "canonical handoff row must persist route_readiness_status=ready immediately after approval, got {readiness_status}"
                        ));
                    }
                    if evaluated_at.is_empty() {
                        gaps.push("canonical handoff row must persist route_readiness_evaluated_at for reconnect-safe inspection".to_owned());
                    }
                    if eligibility_status != "eligible" {
                        gaps.push(format!(
                            "canonical handoff row must derive eligibility_status=eligible from approved fresh readiness + active runtime control, got {eligibility_status}"
                        ));
                    }
                    if hold_reason.is_some() {
                        gaps.push("freshly approved ready handoff must not persist a hold_reason while active and eligible".to_owned());
                    }
                    if runtime_control_mode != "active" {
                        gaps.push(format!(
                            "canonical handoff row must persist runtime_control_mode=active before any explicit stop, got {runtime_control_mode}"
                        ));
                    }
                    if strategy_id.is_some() {
                        gaps.push("canonical handoff row should keep strategy_id empty until a real runtime identity exists".to_owned());
                    }
                }
                None => gaps.push(
                    "direct approval must persist one canonical approved-runtime handoff row keyed by install/proposal/selection identity"
                        .to_owned(),
                ),
            }
        }
    } else {
        gaps.push(format!(
            "missing canonical table {APPROVED_RUNTIME_HANDOFFS_TABLE} for approved runtime handoff persistence"
        ));
    }

    let handoff_event: Option<String> = connection
        .query_row(
            "SELECT payload_json
             FROM event_journal
             WHERE stream_type = ?1 AND stream_id = ?2 AND event_type = ?3
             ORDER BY created_at DESC, event_id DESC
             LIMIT 1",
            [
                STRATEGY_RUNTIME_STREAM_TYPE,
                approved.selection_id.as_str(),
                STRATEGY_RUNTIME_HANDOFF_EVENT,
            ],
            |row| row.get(0),
        )
        .optional()
        .expect("handoff event query should remain inspectable");
    if handoff_event.is_none() {
        gaps.push(
            "event_journal must record strategy_runtime_handoff_persisted when approval becomes a canonical runtime handoff"
                .to_owned(),
        );
    }

    let eligibility_event: Option<String> = connection
        .query_row(
            "SELECT payload_json
             FROM event_journal
             WHERE stream_type = ?1 AND stream_id = ?2 AND event_type = ?3
             ORDER BY created_at DESC, event_id DESC
             LIMIT 1",
            [
                STRATEGY_RUNTIME_STREAM_TYPE,
                approved.selection_id.as_str(),
                STRATEGY_RUNTIME_ELIGIBILITY_EVENT,
            ],
            |row| row.get(0),
        )
        .optional()
        .expect("eligibility event query should remain inspectable");
    if eligibility_event.is_none() {
        gaps.push(
            "event_journal must record strategy_runtime_eligibility_changed so blocked-path diagnostics survive reconnect"
                .to_owned(),
        );
    }

    let reopened_onboarding = inspect_guided_onboarding(GuidedOnboardingInspectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
    })
    .await
    .expect("ready install should remain reopen-inspectable after direct approval");
    if reopened_onboarding.proposal_handoff.is_none() {
        gaps.push(
            "direct reconnect-safe inspection must preserve proposal_handoff after approval-to-runtime persistence"
                .to_owned(),
        );
    }

    let reopened_selection = inspect_strategy_selection(InspectStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
    })
    .await
    .expect("approved selection should remain direct-inspectable after reconnect-style reread");
    if reopened_selection.summary.selection_revision != overridden.summary.selection_revision {
        gaps.push(
            "direct reconnect-safe inspection must reread the current approved selection revision after post-approval overrides"
                .to_owned(),
        );
    }

    assert!(
        gaps.is_empty(),
        "S02 direct approved-runtime handoff contract missing canonical persistence, readiness invalidation, or idempotent approval: {}",
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

async fn routed_daemon_service(
    workspace_root: &Path,
) -> DaemonService<
    BaselinePolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
    a2ex_evm_adapter::NoopEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    let across = FakeAcrossTransport::default();
    let prediction = FakePredictionMarketTransport::default();
    let hyperliquid = FakeHyperliquidTransport::default();
    DaemonService::from_config_with_multi_venue_adapters(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        a2ex_evm_adapter::NoopEvmAdapter,
        AcrossAdapter::with_transport(across.transport(), 0),
        PredictionMarketAdapter::with_transport(prediction.transport()),
        HyperliquidAdapter::with_transport(hyperliquid.transport(), 0),
    )
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

fn expected_route_id(preview: &a2ex_daemon::IntentPreviewResponse) -> String {
    let adapters = preview
        .plan_preview
        .as_ref()
        .map(|plan| {
            plan.steps
                .iter()
                .map(|step| step.adapter.as_str())
                .collect::<Vec<_>>()
                .join(":")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "no_steps".to_owned());
    format!("{:?}:{adapters}", preview.route.route).to_lowercase()
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

#[allow(dead_code)]
async fn read_resource_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Value {
    let response = client
        .read_resource(rmcp::model::ReadResourceRequestParams::new(uri.clone()))
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

fn intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-runtime-handoff-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-runtime-handoff-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-runtime-handoff-1",
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "us-election-2028",
                    "side": "yes",
                    "target_notional_usd": 3000
                },
                "constraints": {
                    "allowed_venues": ["polymarket", "kalshi"],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "normal",
                    "hedge_ratio_bps": 4000
                },
                "funding": {
                    "preferred_asset": "USDC",
                    "source_chain": "base"
                },
                "post_actions": [
                    {"action_type": "hedge", "venue": "hyperliquid"}
                ]
            },
            "rationale": {"summary": "bridge then enter and hedge", "main_risks": ["bridge delay"]},
            "execution_preferences": {"preview_only": false, "allow_fast_path": true}
        }),
    )
}
