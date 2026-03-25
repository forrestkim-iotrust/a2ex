mod support;

use std::sync::Arc;

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, NoopRuntimeSigner};
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
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

#[derive(Default)]
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
async fn failed_multistep_execution_persists_actionable_step_error_state() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let across = FakeAcrossTransport::default();
    across.fail_next_submit("relay timeout one");
    across.fail_next_submit("relay timeout two");
    let service = DaemonService::from_config_with_multi_venue_adapters(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(NoopRuntimeSigner),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        a2ex_evm_adapter::NoopEvmAdapter,
        AcrossAdapter::with_transport(across.transport(), 0),
        PredictionMarketAdapter::with_transport(
            FakePredictionMarketTransport::default().transport(),
        ),
        HyperliquidAdapter::with_transport(FakeHyperliquidTransport::default().transport(), 0),
    );

    let _ = service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    let plan = service
        .plan_intent_request("req-obs-1")
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
        .expect("execution returns report");
    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let steps = repository
        .load_execution_plan_steps(&plan.plan_id)
        .await
        .expect("steps load");
    let execution = repository
        .load_snapshot()
        .await
        .expect("snapshot loads")
        .executions
        .into_iter()
        .find(|execution| execution.execution_id == plan.plan_id)
        .expect("execution state persists");

    assert_eq!(report.status, "failed");
    let bridge_step = steps
        .into_iter()
        .find(|step| step.step_id.ends_with(":bridge"))
        .expect("bridge step exists");
    assert_eq!(bridge_step.status, "failed");
    assert_eq!(bridge_step.attempts, 2);
    assert!(
        bridge_step
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("relay timeout"))
    );
    assert_eq!(execution.status, "failed");
}

fn intent_request() -> a2ex_ipc::JsonRpcRequest<serde_json::Value> {
    a2ex_ipc::JsonRpcRequest::new(
        "req-obs-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-obs-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-multi-obs",
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
