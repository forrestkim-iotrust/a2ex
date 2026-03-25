mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_mcp::A2exSkillMcpServer;
use a2ex_policy::{PolicyDecision, PolicyEvaluator, PolicyInput};
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
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
use support::{
    hyperliquid_harness::FakeHyperliquidTransport,
    skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle},
};
use tempfile::tempdir;

const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";
const TOOL_EVALUATE_ROUTE_READINESS: &str = "readiness.evaluate_route";
const TOOL_APPLY_ROUTE_READINESS_ACTION: &str = "readiness.apply_action";
const TOOL_MATERIALIZE_STRATEGY_SELECTION: &str = "strategy_selection.materialize";
const TOOL_APPROVE_STRATEGY_SELECTION: &str = "strategy_selection.approve";
const TOOL_APPLY_STRATEGY_OVERRIDE: &str = "strategy_selection.apply_override";
const TOOL_STRATEGY_SELECTION_REOPEN: &str = "strategy_selection.reopen";

const RESOURCE_STRATEGY_SELECTION_DIFF_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/diff";
const RESOURCE_STRATEGY_SELECTION_APPROVAL_HISTORY_TEMPLATE: &str = "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/approval-history";
const RESOURCE_STRATEGY_RUNTIME_MONITORING_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/monitoring";

const PROMPT_STRATEGY_SELECTION_DISCUSSION: &str = "operator.strategy_selection_discussion";
const PROMPT_STRATEGY_SELECTION_RECOVERY: &str = "operator.strategy_selection_recovery";
const PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";
const PROMPT_ARGUMENT_PROPOSAL_ID: &str = "proposal_id";
const PROMPT_ARGUMENT_SELECTION_ID: &str = "selection_id";

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

#[derive(Clone, Default)]
struct AllowAllPolicy;

impl PolicyEvaluator for AllowAllPolicy {
    fn evaluate(&self, _input: &PolicyInput) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

#[tokio::test]
async fn strategy_operator_surface_mcp_requires_read_first_reopen_discussion_recovery_and_runtime_identity_refresh()
 {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S03 operator handlers exist");
    let advertised_tools = capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let advertised_resources = capabilities
        .resources
        .iter()
        .map(|resource| resource.uri_template.as_str())
        .collect::<Vec<_>>();
    let advertised_prompts = capabilities
        .prompts
        .iter()
        .map(|prompt| prompt.name.as_str())
        .collect::<Vec<_>>();

    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let first_client = spawn_live_client().await;

    let bootstrap =
        bootstrap_install_live(&first_client, &entry_url, workspace_root.path(), None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id string")
        .to_owned();

    apply_action(
        &first_client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let onboarding_ready = apply_action(
        &first_client,
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
        &first_client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.clone()))]),
    )
    .await;
    let proposal_id = load["session_id"]
        .as_str()
        .expect("session id string")
        .to_owned();
    let proposal = call_tool_json(
        &first_client,
        TOOL_GENERATE_PROPOSAL_PACKET,
        json_map([("session_id", Value::String(proposal_id.clone()))]),
    )
    .await;
    assert_eq!(proposal["proposal_readiness"], "ready");

    let routing_service = routed_daemon_service(workspace_root.path()).await;
    let submit = routing_service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request("req-operator-surface-mcp-1")
        .await
        .expect("preview builds");
    let route_id = expected_route_id(&preview);

    let first_eval = call_tool_json(
        &first_client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-operator-surface-mcp-1".to_owned()),
            ),
        ]),
    )
    .await;
    assert_eq!(
        first_eval["identity"]["request_id"],
        "req-operator-surface-mcp-1"
    );

    let reservations =
        SqliteReservationManager::open(workspace_root.path().join(".a2ex-daemon/state.db"))
            .await
            .expect("reservations open");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-operator-surface-mcp-1".to_owned(),
            execution_id: "req-operator-surface-mcp-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("reservation evidence persists");

    let second_eval = call_tool_json(
        &first_client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-operator-surface-mcp-1".to_owned()),
            ),
        ]),
    )
    .await;
    if second_eval["current_step_key"] != "satisfy_route_approvals" {
        call_tool_json(
            &first_client,
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
        &first_client,
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
        &first_client,
        TOOL_MATERIALIZE_STRATEGY_SELECTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
        ]),
    )
    .await;
    let selection_id = materialized["selection_id"]
        .as_str()
        .expect("selection id string")
        .to_owned();

    let approved = call_tool_json(
        &first_client,
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
                    "note": "approve through the shipped MCP seam before operator recovery"
                }),
            ),
        ]),
    )
    .await;
    assert_eq!(approved["status"], "approved");

    let runtime_service = stateful_runtime_service(workspace_root.path()).await;
    register_strategy(&runtime_service).await;

    let monitoring_uri = format!(
        "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/monitoring"
    );
    let diff_uri = format!(
        "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/diff"
    );
    let approval_history_uri = format!(
        "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/approval-history"
    );

    let mut gaps = Vec::new();
    for required_tool in [TOOL_STRATEGY_SELECTION_REOPEN] {
        if !advertised_tools.contains(&required_tool) {
            gaps.push(format!(
                "initialize() missing advertised tool {required_tool} for same-identity reopen"
            ));
        }
    }
    for required_resource in [
        RESOURCE_STRATEGY_SELECTION_DIFF_TEMPLATE,
        RESOURCE_STRATEGY_SELECTION_APPROVAL_HISTORY_TEMPLATE,
        RESOURCE_STRATEGY_RUNTIME_MONITORING_TEMPLATE,
    ] {
        if !advertised_resources.contains(&required_resource) {
            gaps.push(format!(
                "initialize() missing advertised operator resource template {required_resource}"
            ));
        }
    }
    for required_prompt in [
        PROMPT_STRATEGY_SELECTION_DISCUSSION,
        PROMPT_STRATEGY_SELECTION_RECOVERY,
    ] {
        if !advertised_prompts.contains(&required_prompt) {
            gaps.push(format!(
                "initialize() missing reusable operator prompt {required_prompt}"
            ));
        }
    }

    let thin_override = call_tool_json(
        &first_client,
        TOOL_APPLY_STRATEGY_OVERRIDE,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("selection_id", Value::String(selection_id.clone())),
            (
                "override",
                json!({
                    "key": "approve-max-spread-budget",
                    "value": { "resolution": "approved", "budget_bps": 25 },
                    "rationale": "operator changed a readiness-sensitive assumption after approval"
                }),
            ),
        ]),
    )
    .await;
    if thin_override.get("effective_diff").is_some()
        || thin_override.get("approval_history").is_some()
    {
        gaps.push("strategy_selection.apply_override must stay a thin mutation receipt and force rereads of canonical diff/history resources".to_owned());
    }

    match read_resource_json_result(&first_client, diff_uri.clone()).await {
        Ok(diff) => {
            if diff["selection_id"] != selection_id {
                gaps.push("diff resource must retain the canonical selection identity".to_owned());
            }
            if !diff["changed_override_keys"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item == "approve-max-spread-budget"))
            {
                gaps.push("diff resource must expose changed_override_keys for effective override discussion".to_owned());
            }
            if diff["approval_stale_reason"] != "readiness_sensitive_override" {
                gaps.push("diff resource must expose approval_stale_reason=readiness_sensitive_override after reopen-worthy override".to_owned());
            }
        }
        Err(error) => gaps.push(format!(
            "canonical diff resource must be readable from the shipped MCP surface after approval, got {error}"
        )),
    }

    match read_resource_json_result(&first_client, approval_history_uri.clone()).await {
        Ok(history) => {
            if !history["events"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["event_kind"] == "approved"))
            {
                gaps.push("approval-history resource must retain the prior approval event after reopen-worthy override".to_owned());
            }
            if !history["events"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["event_kind"] == "reopened"))
            {
                gaps.push("approval-history resource must expose a reopened event on the same selection identity".to_owned());
            }
        }
        Err(error) => gaps.push(format!(
            "approval-history resource must be readable from canonical local state, got {error}"
        )),
    }

    let monitoring_before_restart = read_resource_json(&first_client, monitoring_uri.clone()).await;
    if monitoring_before_restart["strategy_id"] != "strategy-lp-1" {
        gaps.push(
            "monitoring resource must refresh strategy_id once a real runtime identity exists"
                .to_owned(),
        );
    }
    if monitoring_before_restart["current_phase"] == "awaiting_runtime_identity" {
        gaps.push(
            "monitoring resource must leave awaiting_runtime_identity after runtime activation"
                .to_owned(),
        );
    }
    if monitoring_before_restart["runtime_identity_refreshed_at"].is_null() {
        gaps.push("monitoring resource must expose runtime_identity_refreshed_at for reconnect-safe freshness checks".to_owned());
    }
    if monitoring_before_restart["hold_reason"] != "approved_selection_revision_stale" {
        gaps.push("monitoring resource must surface approved_selection_revision_stale after reopen-worthy override".to_owned());
    }

    first_client
        .cancel()
        .await
        .expect("first live stdio server should shut down cleanly");

    let reconnected_client = spawn_live_client().await;
    let pre_reopen_diff_error = read_resource_error(&reconnected_client, diff_uri.clone()).await;
    if !(pre_reopen_diff_error.contains("install") || pre_reopen_diff_error.contains("locator")) {
        gaps.push(format!(
            "diff resource must reject reconnect reads before bootstrap reopen repopulates the install locator, got {pre_reopen_diff_error}"
        ));
    }
    let reopened = bootstrap_install_live(
        &reconnected_client,
        &entry_url,
        workspace_root.path(),
        Some(workspace_id),
        Some(install_id.clone()),
    )
    .await;
    if reopened["claim_disposition"] != "reopened" {
        gaps.push(
            "bootstrap reopen must preserve install identity for reconnect-safe operator resources"
                .to_owned(),
        );
    }

    let monitoring_after_restart =
        read_resource_json(&reconnected_client, monitoring_uri.clone()).await;
    if monitoring_after_restart["strategy_id"] != "strategy-lp-1" {
        gaps.push(
            "monitoring resource must preserve refreshed strategy_id after reconnect".to_owned(),
        );
    }
    if monitoring_after_restart["hold_reason"] != "approved_selection_revision_stale" {
        gaps.push(
            "monitoring resource must preserve the stale approval diagnostic after reconnect"
                .to_owned(),
        );
    }

    reconnected_client
        .cancel()
        .await
        .expect("reconnected live stdio server should shut down cleanly");

    assert!(
        gaps.is_empty(),
        "S03 MCP operator-surface contract missing read-first reopen/discussion/recovery surfaces or runtime-identity refresh: {}",
        gaps.join("; ")
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
    a2ex_policy::BaselinePolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
    a2ex_evm_adapter::NoopEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    DaemonService::from_config_with_multi_venue_adapters(
        &config,
        a2ex_policy::BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        a2ex_evm_adapter::NoopEvmAdapter,
        a2ex_across_adapter::AcrossAdapter::with_transport(
            support::across_harness::FakeAcrossTransport::default().transport(),
            0,
        ),
        a2ex_prediction_market_adapter::PredictionMarketAdapter::with_transport(
            support::prediction_market_harness::FakePredictionMarketTransport::default()
                .transport(),
        ),
        a2ex_hyperliquid_adapter::HyperliquidAdapter::with_transport(
            support::hyperliquid_harness::FakeHyperliquidTransport::default().transport(),
            0,
        ),
    )
}

async fn stateful_runtime_service(
    workspace_root: &Path,
) -> DaemonService<
    AllowAllPolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
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
            Arc::new(ApprovingBridge),
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

fn seeded_hyperliquid_harness() -> FakeHyperliquidTransport {
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

async fn register_strategy(
    service: &DaemonService<
        impl PolicyEvaluator,
        SqliteReservationManager,
        RecordingSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
        SimulatedEvmAdapter,
    >,
) {
    let response: JsonRpcResponse<a2ex_daemon::StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
}

fn strategy_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-strategy-operator-surface-mcp",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-operator-surface-mcp",
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

async fn read_resource_json_result(
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

async fn read_resource_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Value {
    read_resource_json_result(client, uri)
        .await
        .unwrap_or_else(|error| panic!("resource should be readable: {error}"))
}

async fn read_resource_error(
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

async fn prompt_text_result(
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

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-operator-surface-mcp-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-operator-surface-mcp-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-operator-surface-mcp-1",
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
