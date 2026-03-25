mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_onboarding::{
    RouteReadinessEvaluationRequest, RouteReadinessInspectionRequest, RouteReadinessStatus,
    evaluate_route_readiness, inspect_guided_onboarding, inspect_route_readiness,
};
use a2ex_policy::BaselinePolicy;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::SqliteReservationManager;
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
};
use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ResourceContents},
    transport::TokioChildProcess,
};
use rusqlite::Connection;
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

#[derive(Default, Clone)]
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
async fn guided_route_readiness_contract_requires_progress_resume_rejection_and_stale_state() {
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

    let inspection =
        inspect_guided_onboarding(a2ex_onboarding::GuidedOnboardingInspectionRequest {
            state_db_path: workspace_root.path().join(".a2ex-daemon/state.db"),
            install_id: install_id.clone(),
        })
        .await
        .expect("ready install should remain inspectable through the direct onboarding seam");
    let handoff = inspection
        .proposal_handoff
        .expect("ready onboarding must expose an explicit proposal handoff");

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

    let service = routed_daemon_service(workspace_root.path()).await;
    let submit = service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = service
        .preview_intent_request("req-route-readiness-1")
        .await
        .expect("preview builds");
    let route_id = expected_route_id(&preview);

    let record = evaluate_route_readiness(RouteReadinessEvaluationRequest {
        state_db_path: workspace_root.path().join(".a2ex-daemon/state.db"),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        request_id: "req-route-readiness-1".to_owned(),
    })
    .await
    .expect("S02 contract should start from one real install→proposal→route readiness evaluation");
    assert_eq!(record.status, RouteReadinessStatus::Incomplete);

    let resumed = inspect_route_readiness(RouteReadinessInspectionRequest {
        state_db_path: workspace_root.path().join(".a2ex-daemon/state.db"),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
    })
    .await
    .expect("route readiness should remain inspectable from canonical state after reevaluation");
    assert_eq!(resumed.identity, record.identity);

    let record_json = serde_json::to_value(&record).expect("record serializes");
    let resumed_json = serde_json::to_value(&resumed).expect("resumed record serializes");

    let connection = Connection::open(workspace_root.path().join(".a2ex-daemon/state.db"))
        .expect("state db should remain inspectable after route readiness evaluation");
    let route_columns = table_columns(&connection, "onboarding_route_readiness");
    let route_row_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM onboarding_route_readiness WHERE install_id = ?1 AND proposal_id = ?2 AND route_id = ?3",
            [install_id.as_str(), proposal_id.as_str(), route_id.as_str()],
            |row| row.get(0),
        )
        .expect("route-scoped readiness row should persist");
    assert_eq!(route_row_count, 1);

    let mut gaps = Vec::new();
    collect_guided_route_gaps("evaluation", &record_json, &mut gaps);
    collect_guided_route_gaps("resume", &resumed_json, &mut gaps);
    require_columns(
        &route_columns,
        &[
            "ordered_steps_json",
            "current_step_key",
            "last_route_rejection_code",
            "last_route_rejection_message",
            "last_route_rejection_at",
            "evaluation_fingerprint",
            "stale_status",
            "stale_reason",
            "stale_detected_at",
        ],
        &mut gaps,
    );

    assert!(
        gaps.is_empty(),
        "S02 direct route-readiness contract missing guided-flow persistence, resume, rejection, or stale invalidation coverage: {}",
        gaps.join("; ")
    );
}

fn collect_guided_route_gaps(label: &str, value: &Value, gaps: &mut Vec<String>) {
    let ordered_steps = value.get("ordered_steps").and_then(Value::as_array);
    if ordered_steps.is_none() {
        gaps.push(format!(
            "{label} missing ordered_steps for route-scoped funding/approval/stale-review guidance"
        ));
    }
    let step_keys = ordered_steps
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| step.get("step_key").and_then(Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for required in [
        "fund_route_capital",
        "satisfy_route_approvals",
        "review_stale_readiness",
    ] {
        if !step_keys.iter().any(|step_key| step_key == &required) {
            gaps.push(format!(
                "{label} missing guided step {required} in ordered route readiness flow"
            ));
        }
    }
    if value.get("current_step_key").and_then(Value::as_str) != Some("fund_route_capital") {
        gaps.push(format!(
            "{label} missing restart-safe current_step_key=fund_route_capital"
        ));
    }
    if value
        .get("recommended_action")
        .and_then(|action| action.get("step_key"))
        .and_then(Value::as_str)
        != Some("fund_route_capital")
    {
        gaps.push(format!(
            "{label} missing recommended_action.step_key=fund_route_capital"
        ));
    }
    if value
        .get("evaluation")
        .and_then(|evaluation| evaluation.get("fingerprint"))
        .and_then(Value::as_str)
        .is_none()
    {
        gaps.push(format!(
            "{label} missing evaluation fingerprint for reevaluation drift detection"
        ));
    }
    if value
        .get("stale")
        .and_then(|stale| stale.get("status"))
        .and_then(Value::as_str)
        .is_none()
    {
        gaps.push(format!(
            "{label} missing stale lifecycle state for reevaluation drift invalidation"
        ));
    }
    if value.get("last_rejection").is_none() {
        gaps.push(format!(
            "{label} missing durable last_rejection surface for out-of-order route actions"
        ));
    }
}

fn require_columns(columns: &BTreeSet<String>, required: &[&str], gaps: &mut Vec<String>) {
    for column in required {
        if !columns.contains(*column) {
            gaps.push(format!(
                "onboarding_route_readiness missing persisted column {column}"
            ));
        }
    }
}

fn table_columns(connection: &Connection, table: &str) -> BTreeSet<String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("pragma statement prepares");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("pragma rows query");
    rows.collect::<Result<BTreeSet<_>, _>>()
        .expect("pragma rows collect")
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
        "req-route-readiness-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-route-readiness-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-route-readiness-1",
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
