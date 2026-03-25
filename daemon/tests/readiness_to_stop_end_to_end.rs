mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::{BaselinePolicy, PolicyDecision, PolicyEvaluator, PolicyInput};
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignedTx, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord, TxSignRequest,
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
const TOOL_EVALUATE_ROUTE_READINESS: &str = "readiness.evaluate_route";
const TOOL_APPLY_ROUTE_READINESS_ACTION: &str = "readiness.apply_action";
const TOOL_RUNTIME_STOP: &str = "runtime.stop";
const TOOL_RUNTIME_CLEAR_STOP: &str = "runtime.clear_stop";

const PROMPT_ROUTE_READINESS_GUIDANCE: &str = "readiness.route_guidance";
const PROMPT_RUNTIME_CONTROL_GUIDANCE: &str = "runtime.control_guidance";
const PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";
const PROMPT_ARGUMENT_PROPOSAL_ID: &str = "proposal_id";
const PROMPT_ARGUMENT_ROUTE_ID: &str = "route_id";

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
            .expect("handoff log lock")
            .push(request.action_kind.clone());
    }
}

#[derive(Default, Clone)]
struct SigningBridge {
    approvals: Arc<Mutex<Vec<String>>>,
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
struct AllowAllPolicy;

impl PolicyEvaluator for AllowAllPolicy {
    fn evaluate(&self, _input: &PolicyInput) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

#[tokio::test]
async fn readiness_to_stop_requires_runtime_handoff_blocked_execution_and_reopen_contract() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let bootstrap =
        bootstrap_install_live(&client, &entry_url, workspace_root.path(), None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id string")
        .to_owned();

    apply_action(
        &client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let onboarding_ready = apply_action(
        &client,
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
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.clone()))]),
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

    let routing_service = routed_daemon_service(workspace_root.path()).await;
    let submit = routing_service
        .submit_intent(intent_request())
        .await
        .expect("submit intent should succeed");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request("req-readiness-stop-1")
        .await
        .expect("preview should build the canonical route plan");
    let route_id = expected_route_id(&preview);

    let first_eval = call_tool_json(
        &client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-readiness-stop-1".to_owned()),
            ),
        ]),
    )
    .await;
    assert_eq!(first_eval["status"], "incomplete");
    assert_eq!(first_eval["current_step_key"], "fund_route_capital");

    let blockers_uri =
        format!("a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/blockers");
    let blockers_before = read_resource_json(&client, blockers_uri.clone()).await;
    assert!(
        blockers_before["blockers"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item["code"] == "capital_evidence_incomplete")),
        "S04 assembled contract must expose capital_evidence_incomplete before reservation-backed reevaluation"
    );

    let reservations =
        SqliteReservationManager::open(workspace_root.path().join(".a2ex-daemon/state.db"))
            .await
            .expect("reservations should open against canonical state.db");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-readiness-stop-1".to_owned(),
            execution_id: "req-readiness-stop-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("held reservation evidence should persist for the same readiness request");

    let mut gaps = Vec::new();

    let second_eval = call_tool_json(
        &client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-readiness-stop-1".to_owned()),
            ),
        ]),
    )
    .await;
    if second_eval["status"] != "ready" {
        gaps.push(
            "reservation-backed reevaluation must flip route readiness status to ready on the same canonical route"
                .to_owned(),
        );
    }

    let mut approval_entry = second_eval.clone();
    if second_eval["current_step_key"] != "satisfy_route_approvals" {
        gaps.push(format!(
            "reservation-backed reevaluation must advance the guided step to satisfy_route_approvals, found {}",
            second_eval["current_step_key"]
        ));
        if second_eval["current_step_key"] == "fund_route_capital" {
            match call_tool_json_result(
                &client,
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
            .await
            {
                Ok(result) => approval_entry = result,
                Err(error) => gaps.push(format!(
                    "route readiness should accept fund_route_capital after reservation evidence is present, got {error}"
                )),
            }
        }
    }

    let approval_step = if approval_entry["current_step_key"] == "satisfy_route_approvals" {
        match call_tool_json_result(
            &client,
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
        .await
        {
            Ok(result) => result,
            Err(error) => {
                gaps.push(format!(
                    "route readiness should accept satisfy_route_approvals once it becomes the current step, got {error}"
                ));
                approval_entry.clone()
            }
        }
    } else {
        gaps.push(format!(
            "guided route readiness should expose satisfy_route_approvals before the final approval completion, found {}",
            approval_entry["current_step_key"]
        ));
        approval_entry.clone()
    };
    if approval_step["status"] != "ready" {
        gaps.push("guided approval completion must keep the route in ready status".to_owned());
    }
    if !approval_step["current_step_key"].is_null() {
        gaps.push(format!(
            "guided approval completion must clear current_step_key after the route is fully ready, found {}",
            approval_step["current_step_key"]
        ));
    }

    let route_guidance = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_ROUTE_READINESS_GUIDANCE).with_arguments(json_map(
                [
                    (
                        PROMPT_ARGUMENT_INSTALL_ID,
                        Value::String(install_id.clone()),
                    ),
                    (
                        PROMPT_ARGUMENT_PROPOSAL_ID,
                        Value::String(proposal_id.clone()),
                    ),
                    (PROMPT_ARGUMENT_ROUTE_ID, Value::String(route_id.clone())),
                ],
            )),
        )
        .await
        .expect("route guidance prompt should render after readiness becomes ready");
    let route_guidance_text = prompt_text(&route_guidance);

    let runtime_service = stateful_runtime_service(workspace_root.path()).await;
    register_strategy(&runtime_service).await;
    let rebalance_command = runtime_service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![a2ex_strategy_runtime::RuntimeWatcherState {
                watcher_key: "w-1".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-12T00:10:00Z".to_owned(),
            }],
            "2026-03-12T00:10:00Z",
        )
        .await
        .expect("runtime should emit a rebalance command before stop control")
        .into_iter()
        .next()
        .expect("stateful runtime should produce one rebalance command");

    let stop = call_tool_json(
        &client,
        TOOL_RUNTIME_STOP,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    assert_eq!(stop["control_mode"], "stopped");
    assert_eq!(stop["autonomy_eligibility"], "blocked");

    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-readiness-stop-2".to_owned(),
            execution_id: "rebalance-readiness-stop-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 310,
        })
        .await
        .expect("runtime reservation should persist before blocked execution attempt");
    let blocked = runtime_service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command,
            "reservation-readiness-stop-2",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:10:05Z",
        )
        .await;
    let blocked_error = match blocked {
        Ok(_) => panic!(
            "S04 assembled contract requires the real daemon path to reject autonomous actions while runtime is stopped"
        ),
        Err(error) => error.to_string(),
    };
    assert!(
        blocked_error.contains("runtime_stopped"),
        "S04 assembled contract requires runtime rejection diagnostics to surface runtime_stopped, got {blocked_error}"
    );

    let failures_uri = format!("a2ex://runtime/control/{install_id}/failures");
    let failures = read_resource_json(&client, failures_uri.clone()).await;
    assert_eq!(failures["last_rejection"]["code"], "runtime_stopped");
    assert_eq!(
        failures["last_rejection"]["attempted_operation"],
        "strategy_rebalance"
    );

    client
        .cancel()
        .await
        .expect("first live stdio server should shut down cleanly");

    let reconnected_client = spawn_live_client().await;
    let status_uri = format!("a2ex://runtime/control/{install_id}/status");
    let route_summary_uri =
        format!("a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/summary");
    let pre_reopen_runtime_error =
        read_resource_error(&reconnected_client, status_uri.clone()).await;
    assert!(
        pre_reopen_runtime_error.contains("install")
            || pre_reopen_runtime_error.contains("locator"),
        "S04 reconnect contract requires install-scoped runtime resources to reject reads until onboarding.bootstrap_install reopens the install, got {pre_reopen_runtime_error}"
    );
    let pre_reopen_readiness_error =
        read_resource_error(&reconnected_client, route_summary_uri.clone()).await;
    assert!(
        pre_reopen_readiness_error.contains("install")
            || pre_reopen_readiness_error.contains("locator"),
        "S04 reconnect contract requires install-scoped readiness resources to reject reads until onboarding.bootstrap_install reopens the install, got {pre_reopen_readiness_error}"
    );

    let reopened = bootstrap_install_live(
        &reconnected_client,
        &entry_url,
        workspace_root.path(),
        Some(workspace_id.clone()),
        Some(install_id.clone()),
    )
    .await;
    assert_eq!(reopened["claim_disposition"], "reopened");

    let status_after_reopen = read_resource_json(&reconnected_client, status_uri.clone()).await;
    assert_eq!(status_after_reopen["control_mode"], "stopped");
    assert_eq!(status_after_reopen["autonomy_eligibility"], "blocked");
    let failures_after_reopen = read_resource_json(&reconnected_client, failures_uri.clone()).await;
    assert_eq!(failures_after_reopen["control_mode"], "stopped");
    assert_eq!(failures_after_reopen["autonomy_eligibility"], "blocked");
    assert_eq!(
        failures_after_reopen["last_rejection"]["code"],
        "runtime_stopped"
    );
    assert_eq!(
        failures_after_reopen["last_rejection"]["attempted_operation"],
        "strategy_rebalance"
    );
    let runtime_guidance_after_reopen = reconnected_client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_RUNTIME_CONTROL_GUIDANCE).with_arguments(json_map(
                [(
                    PROMPT_ARGUMENT_INSTALL_ID,
                    Value::String(install_id.clone()),
                )],
            )),
        )
        .await
        .expect("runtime control guidance prompt should render before clear-stop after reopen");
    let runtime_guidance_after_reopen_text = prompt_text(&runtime_guidance_after_reopen);
    if !(runtime_guidance_after_reopen_text.contains(TOOL_RUNTIME_CLEAR_STOP)
        && runtime_guidance_after_reopen_text.contains(&status_uri)
        && runtime_guidance_after_reopen_text.contains(&failures_uri)
        && runtime_guidance_after_reopen_text.contains("Autonomy eligibility: blocked")
        && runtime_guidance_after_reopen_text.contains("runtime_stopped"))
    {
        gaps.push(
            "runtime.control_guidance must stay reconnect-safe before clear-stop by surfacing blocked stopped state plus canonical status/failures rereads"
                .to_owned(),
        );
    }
    let readiness_after_reopen =
        read_resource_json(&reconnected_client, route_summary_uri.clone()).await;
    assert_eq!(readiness_after_reopen["status"], "ready");

    let clear = call_tool_json(
        &reconnected_client,
        TOOL_RUNTIME_CLEAR_STOP,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    assert_eq!(clear["control_mode"], "active");
    assert_eq!(clear["autonomy_eligibility"], "eligible");

    let failures_after_clear = read_resource_json(&reconnected_client, failures_uri.clone()).await;
    assert_eq!(failures_after_clear["control_mode"], "active");
    assert_eq!(failures_after_clear["autonomy_eligibility"], "eligible");
    assert_eq!(
        failures_after_clear["last_rejection"]["code"],
        "runtime_stopped"
    );
    assert_eq!(
        failures_after_clear["last_rejection"]["attempted_operation"],
        "strategy_rebalance"
    );

    let runtime_guidance = reconnected_client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_RUNTIME_CONTROL_GUIDANCE).with_arguments(json_map(
                [(
                    PROMPT_ARGUMENT_INSTALL_ID,
                    Value::String(install_id.clone()),
                )],
            )),
        )
        .await
        .expect("runtime control guidance prompt should render after clear-stop");
    let runtime_guidance_text = prompt_text(&runtime_guidance);
    if !(runtime_guidance_text.contains(TOOL_RUNTIME_CLEAR_STOP)
        && runtime_guidance_text.contains(&status_uri)
        && runtime_guidance_text.contains(&failures_uri)
        && runtime_guidance_text.contains("Autonomy eligibility: eligible")
        && runtime_guidance_text.contains("runtime_stopped"))
    {
        gaps.push(
            "clear-stop recovery guidance must stay inspectable after reconnect via runtime.control_guidance"
                .to_owned(),
        );
    }

    if !(route_guidance_text.contains(TOOL_RUNTIME_STOP)
        && route_guidance_text.contains(PROMPT_RUNTIME_CONTROL_GUIDANCE)
        && route_guidance_text.contains(&status_uri)
        && route_guidance_text.contains(&failures_uri)
        && route_guidance_text.contains(&format!("install_id={install_id}")))
    {
        gaps.push(
            "fully-ready route guidance must hand off into install-scoped runtime control guidance once approvals are complete"
                .to_owned(),
        );
    }

    reconnected_client
        .cancel()
        .await
        .expect("reconnected live stdio server should shut down cleanly");

    assert!(
        gaps.is_empty(),
        "S04 readiness-to-stop assembly contract missing route-ready progression, runtime rejection visibility, or reconnect-safe handoff guidance: {}",
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
    BaselinePolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
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
            Arc::new(SigningBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
        a2ex_evm_adapter::NoopEvmAdapter,
        AcrossAdapter::with_transport(across.transport(), 0),
        PredictionMarketAdapter::with_transport(prediction.transport()),
        HyperliquidAdapter::with_transport(hyperliquid.transport(), 0),
    )
}

async fn stateful_runtime_service(
    workspace_root: &Path,
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
        a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
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
        "req-strategy-readiness-stop",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-readiness-stop",
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
    call_tool_json_result(client, tool_name, arguments)
        .await
        .unwrap_or_else(|error| panic!("tool {tool_name} should succeed: {error}"))
}

async fn call_tool_json_result(
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

fn prompt_text(prompt: &rmcp::model::GetPromptResult) -> String {
    prompt
        .messages
        .iter()
        .filter_map(|message| match &message.content {
            rmcp::model::PromptMessageContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-readiness-stop-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-readiness-stop-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-readiness-stop-1",
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
