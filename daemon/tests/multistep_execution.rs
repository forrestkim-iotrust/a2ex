mod support;

use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_policy::BaselinePolicy;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignerBridge,
    SignerBridgeRequestRecord,
};
use a2ex_state::StateRepository;
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
            .expect("handoffs lock")
            .push(request.action_kind.clone());
    }
}

#[derive(Default)]
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
async fn planned_execution_retries_falls_back_and_is_idempotent_on_rerun() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let across = FakeAcrossTransport::default();
    across.fail_next_submit("temporary bridge relay timeout");
    let prediction = FakePredictionMarketTransport::default();
    prediction.fail_next_for_venue("polymarket", "primary book unavailable");
    let hyperliquid = FakeHyperliquidTransport::default();
    seed_hedge_sync(&hyperliquid);
    let signer = RecordingSigner::default();
    let bridge = ApprovingBridge::default();
    let service = DaemonService::from_config_with_multi_venue_adapters(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(signer.clone()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(bridge),
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
    assert!(matches!(submit, a2ex_ipc::JsonRpcResponse::Success(_)));
    let plan = service
        .plan_intent_request("req-exec-1")
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

    let first = service
        .execute_planned_intent(
            &plan.plan_id,
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:00:10Z",
        )
        .await
        .expect("execute plan");
    let second = service
        .execute_planned_intent(
            &plan.plan_id,
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:00:20Z",
        )
        .await
        .expect("rerun plan");

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let steps = repository
        .load_execution_plan_steps(&plan.plan_id)
        .await
        .expect("steps load");

    assert_eq!(first.status, "completed");
    assert_eq!(second.status, "completed");
    assert_eq!(across.submits().len(), 2);
    assert_eq!(prediction.requests().len(), 2);
    assert_eq!(signer.handoffs.lock().expect("handoffs lock").len(), 5);
    assert!(steps.iter().any(|step| step.step_id.ends_with(":bridge")
        && step.attempts == 2
        && step.status == "settled"));
    assert!(steps.iter().any(|step| {
        step.step_id.ends_with(":entry")
            && step.status == "filled"
            && step
                .metadata_json
                .as_deref()
                .is_some_and(|json| json.contains("kalshi"))
    }));
    assert!(
        steps
            .iter()
            .any(|step| step.step_id.ends_with(":hedge") && step.status == "filled")
    );
}

fn seed_hedge_sync(harness: &FakeHyperliquidTransport) {
    harness.seed_open_orders(vec![HyperliquidOpenOrder {
        order_id: 77,
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
        order_id: 77,
        status: "filled".to_owned(),
        filled_size: "1.0".to_owned(),
    });
    harness.seed_user_fills(vec![HyperliquidUserFill {
        order_id: 77,
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
        position_value: "1200".to_owned(),
    }]);
}

fn intent_request() -> a2ex_ipc::JsonRpcRequest<serde_json::Value> {
    a2ex_ipc::JsonRpcRequest::new(
        "req-exec-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-exec-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-multi-1",
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
