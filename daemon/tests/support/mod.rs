pub mod across_harness;
pub mod anvil_harness;
pub mod hyperliquid_harness;
pub mod openclaw_harness;
pub mod prediction_market_harness;
pub mod skill_bundle_harness;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{
    DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff, StrategyRegistrationReceipt,
};
use a2ex_evm_adapter::{NoopEvmAdapter, SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::{BaselinePolicy, PolicyDecision, PolicyEvaluator, PolicyInput};
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignedTx, SignerBridge, SignerBridgeError,
    SignerBridgeRequestRecord, TxSignRequest,
};
use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams, ResourceContents,
    },
    transport::TokioChildProcess,
};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

use crate::support::{
    across_harness::FakeAcrossTransport,
    hyperliquid_harness::FakeHyperliquidTransport,
    prediction_market_harness::FakePredictionMarketTransport,
    skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle},
};

pub const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
pub const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
pub const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
pub const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";
pub const TOOL_EVALUATE_ROUTE_READINESS: &str = "readiness.evaluate_route";
pub const TOOL_APPLY_ROUTE_READINESS_ACTION: &str = "readiness.apply_action";
pub const TOOL_MATERIALIZE_STRATEGY_SELECTION: &str = "strategy_selection.materialize";
pub const TOOL_APPLY_STRATEGY_OVERRIDE: &str = "strategy_selection.apply_override";
pub const TOOL_APPROVE_STRATEGY_SELECTION: &str = "strategy_selection.approve";
pub const TOOL_STRATEGY_SELECTION_REOPEN: &str = "strategy_selection.reopen";
pub const TOOL_RUNTIME_STOP: &str = "runtime.stop";
pub const TOOL_RUNTIME_PAUSE: &str = "runtime.pause";
pub const TOOL_RUNTIME_CLEAR_STOP: &str = "runtime.clear_stop";

pub const PROMPT_RUNTIME_CONTROL_GUIDANCE: &str = "runtime.control_guidance";
pub const PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";
pub const PROMPT_ARGUMENT_PROPOSAL_ID: &str = "proposal_id";
pub const PROMPT_ARGUMENT_SELECTION_ID: &str = "selection_id";
pub const PROMPT_ARGUMENT_ROUTE_ID: &str = "route_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedRuntimeSelectionFixture {
    pub install_id: String,
    pub workspace_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub route_id: String,
    pub request_id: String,
}

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
pub struct RecordingSigner {
    handoffs: Arc<Mutex<Vec<String>>>,
}

impl RecordingSigner {
    pub fn handoffs(&self) -> Vec<String> {
        self.handoffs.lock().expect("handoff log lock").clone()
    }
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, request: &ExecutionRequest) {
        self.handoffs
            .lock()
            .expect("handoff log lock")
            .push(request.action_kind.clone());
    }
}

#[derive(Default, Clone)]
pub struct SigningBridge {
    approvals: Arc<Mutex<Vec<String>>>,
}

impl SigningBridge {
    pub fn approvals(&self) -> Vec<String> {
        self.approvals.lock().expect("approval log lock").clone()
    }
}

#[async_trait]
impl SignerBridge for SigningBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        self.approvals
            .lock()
            .expect("approval log lock")
            .push(req.action_kind.clone());
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }

    async fn sign_transaction(&self, req: TxSignRequest) -> Result<SignedTx, SignerBridgeError> {
        Ok(SignedTx { bytes: req.payload })
    }
}

#[derive(Clone, Default)]
pub struct AllowAllPolicy;

impl PolicyEvaluator for AllowAllPolicy {
    fn evaluate(&self, _input: &PolicyInput) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

pub async fn ready_path_harness() -> SkillBundleHarness {
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

pub async fn routed_daemon_service(
    workspace_root: &Path,
) -> DaemonService<
    BaselinePolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
    NoopEvmAdapter,
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
            Arc::new(SigningBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
        NoopEvmAdapter,
        AcrossAdapter::with_transport(across.transport(), 0),
        PredictionMarketAdapter::with_transport(prediction.transport()),
        HyperliquidAdapter::with_transport(hyperliquid.transport(), 0),
    )
}

pub async fn prepare_approved_runtime_selection(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    workspace_root: &Path,
    entry_url: &str,
    request_id: &str,
    intent_id: &str,
) -> ApprovedRuntimeSelectionFixture {
    let bootstrap = bootstrap_install_live(client, entry_url, workspace_root, None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id")
        .to_owned();

    apply_onboarding_action(
        client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let onboarding_ready = apply_onboarding_action(
        client,
        &install_id,
        json!({
            "kind": "resolve_owner_decision",
            "step_key": "approve-max-spread-budget",
            "resolution": "approved"
        }),
    )
    .await;
    assert_eq!(onboarding_ready["aggregate_status"], "ready");

    let load = call_tool_json(
        client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.to_owned()))]),
    )
    .await;
    let proposal_id = load["session_id"].as_str().expect("proposal id").to_owned();
    let proposal = call_tool_json(
        client,
        TOOL_GENERATE_PROPOSAL_PACKET,
        json_map([("session_id", Value::String(proposal_id.clone()))]),
    )
    .await;
    assert_eq!(proposal["proposal_readiness"], "ready");

    let routing_service = routed_daemon_service(workspace_root).await;
    let submit = routing_service
        .submit_intent(intent_request(request_id, intent_id))
        .await
        .expect("submit intent should succeed");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request(request_id)
        .await
        .expect("preview should succeed");
    let route_id = expected_route_id(&preview);

    let first_eval = call_tool_json(
        client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            ("request_id", Value::String(request_id.to_owned())),
        ]),
    )
    .await;
    assert_eq!(first_eval["identity"]["request_id"], request_id);

    let reservations = SqliteReservationManager::open(workspace_root.join(".a2ex-daemon/state.db"))
        .await
        .expect("reservations should open");
    reservations
        .hold(ReservationRequest {
            reservation_id: format!("reservation-{request_id}-route"),
            execution_id: request_id.to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("route reservation should persist");

    let second_eval = call_tool_json(
        client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            ("request_id", Value::String(request_id.to_owned())),
        ]),
    )
    .await;
    if second_eval["current_step_key"] != "satisfy_route_approvals" {
        call_tool_json(
            client,
            TOOL_APPLY_ROUTE_READINESS_ACTION,
            json_map([
                ("install_id", Value::String(install_id.clone())),
                ("proposal_id", Value::String(proposal_id.clone())),
                ("route_id", Value::String(route_id.clone())),
                (
                    "action",
                    json!({ "kind": "complete_step", "step_key": "fund_route_capital" }),
                ),
            ]),
        )
        .await;
    }
    let ready_route = call_tool_json(
        client,
        TOOL_APPLY_ROUTE_READINESS_ACTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "action",
                json!({ "kind": "complete_step", "step_key": "satisfy_route_approvals" }),
            ),
        ]),
    )
    .await;
    assert_eq!(ready_route["status"], "ready");

    let materialized = call_tool_json(
        client,
        TOOL_MATERIALIZE_STRATEGY_SELECTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
        ]),
    )
    .await;
    let selection_id = materialized["selection_id"]
        .as_str()
        .expect("selection id")
        .to_owned();

    let approved = call_tool_json(
        client,
        TOOL_APPROVE_STRATEGY_SELECTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("selection_id", Value::String(selection_id.clone())),
            ("expected_selection_revision", Value::from(1)),
            (
                "approval",
                json!({
                    "approved_by": "owner",
                    "note": "approve through the canonical test setup seam"
                }),
            ),
        ]),
    )
    .await;
    assert_eq!(approved["status"], "approved");

    ApprovedRuntimeSelectionFixture {
        install_id,
        workspace_id,
        proposal_id,
        selection_id,
        route_id,
        request_id: request_id.to_owned(),
    }
}

pub fn workspace_state_db_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".a2ex-daemon/state.db")
}

pub async fn fresh_workspace() -> TempDir {
    TempDir::new().expect("workspace tempdir")
}

pub async fn stateful_runtime_service(
    workspace_root: &Path,
    outcome: SimulatedOutcome,
) -> DaemonService<
    AllowAllPolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
    SimulatedEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    let harness = seeded_hyperliquid_harness();
    DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        AllowAllPolicy,
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations clone"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 10,
            confirmation_depth: 1,
            outcome,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    )
}

pub async fn rejected_runtime_service(
    workspace_root: &Path,
) -> DaemonService<
    AllowAllPolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
    SimulatedEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    let harness = FakeHyperliquidTransport::default();
    harness.seed_open_orders(Vec::new());
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: "rejected".to_owned(),
        filled_size: "0.0".to_owned(),
    });
    harness.seed_user_fills(Vec::new());
    harness.seed_positions(Vec::new());

    DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        AllowAllPolicy,
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations clone"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 10,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    )
}

pub fn seeded_hyperliquid_harness() -> FakeHyperliquidTransport {
    let harness = FakeHyperliquidTransport::default();
    harness.seed_open_orders(vec![HyperliquidOpenOrder {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        is_buy: false,
        price: "2412.7".to_owned(),
        size: "0.5".to_owned(),
        reduce_only: false,
        status: "resting".to_owned(),
        client_order_id: Some("hl-strategy-lp-1-1".to_owned()),
    }]);
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: "filled".to_owned(),
        filled_size: "0.5".to_owned(),
    });
    harness.seed_user_fills(vec![HyperliquidUserFill {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "0.5".to_owned(),
        price: "2412.7".to_owned(),
        side: "sell".to_owned(),
        filled_at: "2026-03-12T00:10:01Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "-0.5".to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);
    harness
}

pub async fn register_strategy(
    service: &DaemonService<
        impl PolicyEvaluator,
        SqliteReservationManager,
        RecordingSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
        SimulatedEvmAdapter,
    >,
) {
    let response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
}

pub fn strategy_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-strategy-proposal-to-autonomy",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-proposal-to-autonomy",
            "request_kind": "strategy",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:09:00Z",
            "payload": {
                "strategy_id": "strategy-lp-1",
                "strategy_type": "stateful_hedge",
                "watchers": [{"watcher_type": "lp_position", "source": "local", "target": "TOKEN/USDT"}],
                "trigger_rules": [{"trigger_type": "drift_threshold", "metric": "delta_exposure_pct", "operator": ">", "value": "0.02", "cooldown_sec": 10}],
                "calculation_model": {"model_type": "delta_neutral_lp", "inputs": ["lp_token_balance"]},
                "action_templates": [{"action_type": "adjust_hedge", "venue": "hyperliquid", "instrument": "TOKEN-PERP", "target": "delta_neutral"}],
                "constraints": {"min_order_usd": 100, "max_slippage_bps": 40, "max_rebalances_per_hour": 60},
                "unwind_rules": [{"condition": "manual_stop"}]
            },
            "rationale": {"summary": "Keep delta neutral.", "main_risks": []},
            "execution_preferences": {"preview_only": false, "allow_fast_path": false}
        }),
    )
}

pub fn intent_request(request_id: &str, intent_id: &str) -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        request_id,
        "daemon.submitIntent",
        json!({
            "request_id": request_id,
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": intent_id,
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

pub fn expected_route_id(preview: &a2ex_daemon::IntentPreviewResponse) -> String {
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

pub async fn spawn_live_client() -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
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

pub async fn bootstrap_install_live(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_url: &str,
    workspace_root: &Path,
    expected_workspace_id: Option<String>,
    expected_install_id: Option<String>,
) -> Value {
    let mut arguments = json_map([
        ("install_url", Value::String(install_url.to_owned())),
        (
            "workspace_root",
            Value::String(workspace_root.display().to_string()),
        ),
    ]);
    if let Some(expected_workspace_id) = expected_workspace_id {
        arguments.insert(
            "expected_workspace_id".to_owned(),
            Value::String(expected_workspace_id),
        );
    }
    if let Some(expected_install_id) = expected_install_id {
        arguments.insert(
            "expected_install_id".to_owned(),
            Value::String(expected_install_id),
        );
    }
    call_tool_json(client, TOOL_BOOTSTRAP_INSTALL, arguments).await
}

pub async fn apply_onboarding_action(
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

pub async fn call_tool_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool_name: &str,
    arguments: Map<String, Value>,
) -> Value {
    call_tool_json_result(client, tool_name, arguments)
        .await
        .unwrap_or_else(|error| panic!("tool {tool_name} should succeed: {error}"))
}

pub async fn call_tool_json_result(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool_name: &str,
    arguments: Map<String, Value>,
) -> Result<Value, String> {
    client
        .call_tool(CallToolRequestParams::new(tool_name.to_owned()).with_arguments(arguments))
        .await
        .map_err(|error| error.to_string())
        .map(|response| {
            response
                .structured_content
                .expect("tool result should include structured content")
        })
}

pub async fn read_resource_json_result(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Result<Value, String> {
    let response = client
        .read_resource(ReadResourceRequestParams::new(uri.clone()))
        .await
        .map_err(|error| error.to_string())?;
    let text = match &response.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text,
        other => return Err(format!("expected text resource contents, got {other:?}")),
    };
    serde_json::from_str(text).map_err(|error| error.to_string())
}

pub async fn read_resource_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Value {
    read_resource_json_result(client, uri)
        .await
        .unwrap_or_else(|error| panic!("resource should be readable: {error}"))
}

pub async fn read_resource_error(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> String {
    match client
        .read_resource(ReadResourceRequestParams::new(uri.clone()))
        .await
    {
        Ok(response) => panic!(
            "resource {uri} should have failed before reopen, got {:?}",
            response.contents
        ),
        Err(error) => error.to_string(),
    }
}

pub async fn prompt_text_result(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    prompt_name: &str,
    arguments: Map<String, Value>,
) -> Result<String, String> {
    client
        .get_prompt(GetPromptRequestParams::new(prompt_name).with_arguments(arguments))
        .await
        .map(|prompt| {
            prompt
                .messages
                .iter()
                .filter_map(|message| match &message.content {
                    rmcp::model::PromptMessageContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .map_err(|error| error.to_string())
}

pub fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}
