mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_onboarding::{
    RouteReadinessEvaluationRequest, RouteReadinessStatus, evaluate_route_readiness,
    inspect_guided_onboarding, inspect_route_readiness,
};
use a2ex_policy::BaselinePolicy;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::SqliteReservationManager;
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
};
use a2ex_skill_bundle::ProposalQuantitativeCompleteness;
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
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, a2ex_signer_bridge::SignerBridgeError> {
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }
}

#[tokio::test]
async fn route_readiness_projection_locks_identity_capital_truth_and_typed_approvals() {
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
    let human = service
        .human_request_support("req-route-readiness-1")
        .await
        .expect("human support builds");

    assert_eq!(human.capital_required_usd, 3_000);
    assert_eq!(preview.capital_support.required_capital_usd, 3_000);
    assert_eq!(
        preview.capital_support.completeness,
        ProposalQuantitativeCompleteness::Unknown
    );
    assert!(
        preview
            .approval_requirements
            .iter()
            .any(|r| r.venue == "across"
                && r.asset.as_deref() == Some("USDC")
                && r.chain.as_deref() == Some("base")
                && r.context.as_deref() == Some("bridge_submit")
                && r.auth_summary.contains("signed locally"))
    );
    assert!(
        preview
            .approval_requirements
            .iter()
            .any(|r| r.venue == "kalshi"
                && r.context.as_deref() == Some("entry_order_submit")
                && r.auth_summary.contains("Local API key auth"))
    );
    assert_eq!(
        human.capital_support.completeness,
        ProposalQuantitativeCompleteness::Unknown
    );
    assert!(
        human
            .approvals_needed
            .iter()
            .any(|r| r.venue == "hyperliquid"
                && r.context.as_deref() == Some("hedge_order_submit")
                && r.auth_summary.contains("Locally-signed exchange payloads"))
    );

    let route_id = expected_route_id(&preview);
    let record = evaluate_route_readiness(RouteReadinessEvaluationRequest {
        state_db_path: workspace_root.path().join(".a2ex-daemon/state.db"),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        request_id: "req-route-readiness-1".to_owned(),
    })
    .await
    .expect(
        "S01 must evaluate one concrete route from the real onboarding→proposal handoff and daemon preview truth",
    );

    assert_eq!(record.identity.install_id, install_id);
    assert_eq!(record.identity.proposal_id, proposal_id);
    assert_eq!(record.identity.route_id, route_id);
    assert_eq!(record.identity.request_id, "req-route-readiness-1");
    assert_eq!(record.status, RouteReadinessStatus::Incomplete);
    assert_eq!(record.capital.required_capital_usd, Some(3_000));
    assert_eq!(
        record.capital.completeness,
        ProposalQuantitativeCompleteness::Unknown,
        "S01 must keep route capital incomplete when available or reserved capital evidence is absent instead of guessing sufficiency from target notional alone"
    );
    assert_eq!(record.capital.available_capital_usd, None);
    assert_eq!(record.capital.reserved_capital_usd, None);
    assert!(
        record.capital.reason.contains("missing") || record.capital.reason.contains("unknown"),
        "capital readiness must explain why sufficiency is incomplete"
    );
    assert!(
        record.approvals.iter().any(|tuple| {
            tuple.venue == "across"
                && tuple.approval_type == "erc20_allowance"
                && tuple.asset.as_deref() == Some("USDC")
                && tuple.chain.as_deref() == Some("base")
                && tuple.context.as_deref() == Some("bridge_submit")
                && tuple.required
                && tuple.auth_summary.contains("signed locally")
        }),
        "route readiness must expose venue-specific approval tuples instead of a single generic approved boolean"
    );
    assert!(
        record.approvals.iter().any(|tuple| {
            tuple.venue == "kalshi"
                && tuple.approval_type == "api_request_signature"
                && tuple.context.as_deref() == Some("entry_order_submit")
                && tuple.required
                && tuple.auth_summary.contains("Local API key auth")
        }),
        "route readiness must preserve the concrete entry-venue approval tuple"
    );
    assert!(
        record.approvals.iter().any(|tuple| {
            tuple.venue == "hyperliquid"
                && tuple.approval_type == "exchange_order_signature"
                && tuple.context.as_deref() == Some("hedge_order_submit")
                && tuple.required
                && tuple
                    .auth_summary
                    .contains("Locally-signed exchange payloads")
        }),
        "route readiness must preserve the hedge approval tuple separately from venue entry auth"
    );
    assert!(
        record
            .blockers
            .iter()
            .any(|blocker| blocker.code == "capital_evidence_incomplete"),
        "blocked or incomplete readiness must keep blocker provenance visible after evaluation"
    );
    assert_eq!(
        record
            .recommended_owner_action
            .as_ref()
            .map(|action| action.kind.as_str()),
        Some("fund_route_capital"),
        "route readiness must explain the next owner action instead of returning only a failed boolean"
    );

    let persisted = inspect_route_readiness(a2ex_onboarding::RouteReadinessInspectionRequest {
        state_db_path: workspace_root.path().join(".a2ex-daemon/state.db"),
        install_id: record.identity.install_id.clone(),
        proposal_id: record.identity.proposal_id.clone(),
        route_id: record.identity.route_id.clone(),
    })
    .await
    .expect(
        "S01 must keep the blocked or incomplete route-readiness record inspectable after evaluation",
    );
    assert_eq!(persisted.identity, record.identity);
    assert_eq!(persisted.status, RouteReadinessStatus::Incomplete);
    assert_eq!(
        persisted.capital.completeness,
        ProposalQuantitativeCompleteness::Unknown
    );
    assert!(
        persisted
            .blockers
            .iter()
            .any(|blocker| blocker.code == "capital_evidence_incomplete")
    );

    let connection = Connection::open(workspace_root.path().join(".a2ex-daemon/state.db"))
        .expect("state db should remain inspectable after route readiness evaluation");
    let (status, stored_request_id, blockers_json, owner_action_json) = connection
        .query_row(
            "SELECT status, request_id, blockers_json, recommended_owner_action_json
             FROM onboarding_route_readiness
             WHERE install_id = ?1 AND proposal_id = ?2 AND route_id = ?3",
            [install_id.as_str(), proposal_id.as_str(), route_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .expect("canonical route-readiness row must persist in state.db for later inspection");
    assert_eq!(status, "incomplete");
    assert_eq!(stored_request_id, "req-route-readiness-1");
    assert!(blockers_json.contains("capital_evidence_incomplete"));
    assert!(
        owner_action_json
            .as_deref()
            .is_some_and(|value| value.contains("fund_route_capital"))
    );
    assert!(
        owner_action_json
            .as_deref()
            .is_some_and(|value| !value.contains("credential_id"))
    );
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
