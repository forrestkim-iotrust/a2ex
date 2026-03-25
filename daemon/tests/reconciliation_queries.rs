mod support;

use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{
    DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff, StrategyRegistrationReceipt,
};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignerBridge,
    SignerBridgeRequestRecord,
};
use a2ex_strategy_runtime::RuntimeWatcherState;
use async_trait::async_trait;
use serde_json::json;
use support::across_harness::FakeAcrossTransport;
use support::hyperliquid_harness::FakeHyperliquidTransport;
use support::prediction_market_harness::FakePredictionMarketTransport;
use tempfile::tempdir;

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
struct ApprovingBridge {
    approvals: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl SignerBridge for ApprovingBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, a2ex_signer_bridge::SignerBridgeError> {
        self.approvals
            .lock()
            .expect("approvals lock")
            .push(req.action_kind.clone());
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }
}

#[tokio::test]
async fn preview_and_human_support_return_structured_agent_handoff_data() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner::default()),
    );

    let submit = service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));

    let preview = service
        .preview_intent_request("req-reconcile-1")
        .await
        .expect("preview builds");
    let human = service
        .human_request_support("req-reconcile-1")
        .await
        .expect("human support builds");

    assert_eq!(preview.request_id, "req-reconcile-1");
    assert_eq!(
        preview.route.route,
        a2ex_control::RouteTarget::PlannedExecution
    );
    assert_eq!(
        preview
            .plan_preview
            .as_ref()
            .expect("plan preview")
            .steps
            .len(),
        3
    );
    assert_eq!(preview.capital_support.required_capital_usd, 3_000);
    assert_eq!(
        preview.capital_support.completeness,
        a2ex_skill_bundle::ProposalQuantitativeCompleteness::Unknown
    );
    assert!(
        preview
            .approval_requirements
            .iter()
            .any(|requirement| requirement.venue == "across"
                && requirement.asset.as_deref() == Some("USDC")
                && requirement.chain.as_deref() == Some("base")
                && requirement.context.as_deref() == Some("bridge_submit"))
    );
    assert!(
        preview
            .approval_requirements
            .iter()
            .any(|requirement| requirement.venue == "kalshi"
                && requirement.context.as_deref() == Some("entry_order_submit")
                && requirement.auth_summary.contains("Local API key auth"))
    );

    assert_eq!(human.capital_required_usd, 3_000);
    assert_eq!(
        human.capital_support.completeness,
        a2ex_skill_bundle::ProposalQuantitativeCompleteness::Unknown
    );
    assert!(
        human
            .justification_facts
            .iter()
            .any(|fact| fact.contains("Planned steps"))
    );
    assert!(human.approvals_needed.iter().any(|requirement| {
        requirement.venue == "hyperliquid"
            && requirement.context.as_deref() == Some("hedge_order_submit")
            && requirement
                .auth_summary
                .contains("Locally-signed exchange payloads")
    }));
}

#[tokio::test]
async fn reconciliation_persists_expected_vs_actual_and_execution_query_returns_analytics() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let across = FakeAcrossTransport::default();
    let prediction = FakePredictionMarketTransport::default();
    let hyperliquid = FakeHyperliquidTransport::default();
    seed_plan_hedge_sync(&hyperliquid);
    let service = DaemonService::from_config_with_multi_venue_adapters(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
        a2ex_evm_adapter::NoopEvmAdapter,
        AcrossAdapter::with_transport(across.transport(), 0),
        PredictionMarketAdapter::with_transport(prediction.transport()),
        HyperliquidAdapter::with_transport(hyperliquid.transport(), 0),
    );

    let submit = service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let plan = service
        .plan_intent_request("req-reconcile-1")
        .await
        .expect("plan intent");
    SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations reopen")
        .hold(ReservationRequest {
            reservation_id: plan.request_id.clone(),
            execution_id: plan.plan_id.clone(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("hold reservation");

    let report = service
        .execute_planned_intent(
            &plan.plan_id,
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:00:10Z",
        )
        .await
        .expect("execute plan");
    assert_eq!(report.status, "completed");

    let reconciliation = service
        .reconcile_execution(&plan.plan_id, "2026-03-12T00:00:11Z")
        .await
        .expect("reconcile execution");
    let query = service
        .query_execution_state(&plan.plan_id)
        .await
        .expect("query execution state");

    assert_eq!(reconciliation.balances[0].expected_amount_usd, 3_000);
    assert_eq!(reconciliation.balances[0].actual_amount_usd, 3_000);
    assert_eq!(reconciliation.fills[0].actual_fill_usd, 1_800);
    assert_eq!(reconciliation.positions[0].expected_position_usd, 1_200);
    assert_eq!(reconciliation.positions[0].actual_position_usd, 1_000);
    assert_eq!(reconciliation.residual_exposure_usd, 200);
    assert!(reconciliation.rebalance_required);

    assert_eq!(
        query.execution.as_ref().expect("execution").status,
        "completed"
    );
    assert_eq!(query.steps.len(), 3);
    assert_eq!(
        query
            .analytics
            .as_ref()
            .expect("analytics projection")
            .status,
        "completed"
    );
    assert_eq!(
        query
            .reconciliation
            .as_ref()
            .expect("reconciliation state")
            .residual_exposure_usd,
        200
    );
    assert!(
        query
            .journal
            .iter()
            .any(|entry| entry.event_type == "execution_state_changed")
    );
}

#[tokio::test]
async fn strategy_state_query_returns_runtime_snapshot_and_live_hedge_truth() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let harness = FakeHyperliquidTransport::default();
    seed_strategy_hedge_sync(&harness);
    let service = DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 42,
            confirmation_depth: 2,
            outcome: SimulatedOutcome::Confirmed,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    );

    register_strategy(&service).await;
    SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations reopen")
        .hold(ReservationRequest {
            reservation_id: "reservation-strategy-1".to_owned(),
            execution_id: "rebalance".to_owned(),
            asset: "USDC".to_owned(),
            amount: 310,
        })
        .await
        .expect("hold reservation");
    let commands = service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-1".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-11T00:00:10Z".to_owned(),
            }],
            "2026-03-11T00:00:10Z",
        )
        .await
        .expect("evaluate strategy");
    service
        .execute_stateful_hedge(
            "strategy-lp-1",
            commands[0].clone(),
            "reservation-strategy-1",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-11T00:00:11Z",
        )
        .await
        .expect("execute hedge");

    let query = service
        .query_strategy_state("strategy-lp-1")
        .await
        .expect("query strategy state");

    assert_eq!(query.strategy.strategy_id, "strategy-lp-1");
    assert_eq!(
        query.route_decision.as_ref().expect("route").route.route,
        a2ex_control::RouteTarget::StatefulRuntime
    );
    assert_eq!(
        query
            .recovery
            .as_ref()
            .and_then(|snapshot| snapshot.pending_hedge.as_ref())
            .expect("pending hedge")
            .status,
        "filled"
    );
    assert_eq!(
        query
            .live_hedge_sync
            .as_ref()
            .expect("live sync")
            .positions
            .len(),
        1
    );
}

fn seed_plan_hedge_sync(harness: &FakeHyperliquidTransport) {
    harness.seed_open_orders(vec![HyperliquidOpenOrder {
        order_id: 91,
        asset: 0,
        instrument: "RELATED-PERP".to_owned(),
        is_buy: true,
        price: "1200".to_owned(),
        size: "1.0".to_owned(),
        reduce_only: false,
        status: "resting".to_owned(),
        client_order_id: Some("ignored".to_owned()),
    }]);
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: "filled".to_owned(),
        filled_size: "1.0".to_owned(),
    });
    harness.seed_user_fills(vec![HyperliquidUserFill {
        order_id: 91,
        asset: 0,
        instrument: "RELATED-PERP".to_owned(),
        size: "1.0".to_owned(),
        price: "1200".to_owned(),
        side: "buy".to_owned(),
        filled_at: "2026-03-12T00:00:10Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 0,
        instrument: "RELATED-PERP".to_owned(),
        size: "1.0".to_owned(),
        entry_price: "1200".to_owned(),
        position_value: "1000".to_owned(),
    }]);
}

fn seed_strategy_hedge_sync(harness: &FakeHyperliquidTransport) {
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
        filled_at: "2026-03-11T00:00:11Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "-0.5".to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);
}

async fn register_strategy(
    service: &DaemonService<
        impl a2ex_policy::PolicyEvaluator,
        SqliteReservationManager,
        RecordingSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
        SimulatedEvmAdapter,
    >,
) {
    let response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
}

fn intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-reconcile-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-reconcile-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-reconcile-1",
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

fn strategy_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-strategy-1",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-1",
            "request_kind": "strategy",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
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
