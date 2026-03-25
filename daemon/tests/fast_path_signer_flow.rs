use std::sync::{Arc, Mutex};

use a2ex_daemon::{DaemonConfig, DaemonService, SignerHandoff};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome, TxLifecycleStatus};
use a2ex_fast_path::PreparedFastAction;
use a2ex_ipc::JsonRpcRequest;
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignedTx, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord, TxSignRequest,
};
use tempfile::tempdir;

#[derive(Default)]
struct RecordingSigner {
    handoffs: Mutex<Vec<String>>,
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, request: &a2ex_daemon::ExecutionRequest) {
        self.handoffs
            .lock()
            .expect("handoffs lock")
            .push(request.action_id.clone());
    }
}

#[derive(Default)]
struct RecordingBridge {
    approvals: Mutex<Vec<ApprovalRequest>>,
    signatures: Mutex<Vec<TxSignRequest>>,
}

#[async_trait::async_trait]
impl SignerBridge for RecordingBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        self.approvals
            .lock()
            .expect("approvals lock")
            .push(req.clone());
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }

    async fn sign_transaction(&self, req: TxSignRequest) -> Result<SignedTx, SignerBridgeError> {
        self.signatures
            .lock()
            .expect("signatures lock")
            .push(req.clone());
        Ok(SignedTx { bytes: req.payload })
    }
}

#[tokio::test]
async fn fast_path_reservation_and_signer_flow() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-fast".to_owned(),
            execution_id: "exec-fast".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let bridge = Arc::new(RecordingBridge::default());
    let signer_bridge = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        bridge.clone(),
        LocalPeerValidator::strict_local_only(),
    );
    let signer = Arc::new(RecordingSigner::default());
    let service = DaemonService::from_config_with_fast_path(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        signer.clone(),
        signer_bridge,
        SimulatedEvmAdapter {
            block_number: 7,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
    );

    service
        .submit_intent(fast_intent_request())
        .await
        .expect("intent persists");
    let prepared: PreparedFastAction = service
        .prepare_fast_path_action("req-fast-flow", "reservation-fast")
        .await
        .expect("prepared fast path action");

    let report = service
        .execute_fast_path_action(&prepared, LocalPeerIdentity::for_tests(true, true))
        .await
        .expect("fast path executes");

    assert_eq!(
        report.terminal_status(),
        Some(&TxLifecycleStatus::Confirmed)
    );
    assert_eq!(bridge.approvals.lock().expect("approvals lock").len(), 1);
    assert_eq!(bridge.signatures.lock().expect("signatures lock").len(), 1);
    assert_eq!(signer.handoffs.lock().expect("handoffs lock").len(), 1);
}

fn fast_intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-fast-flow",
        "daemon.submitIntent",
        serde_json::json!({
            "request_id": "req-fast-flow",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_id": "intent-fast-flow",
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "market-fast",
                    "side": "yes",
                    "target_notional_usd": 25
                },
                "constraints": {
                    "allowed_venues": ["polymarket"],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "high"
                },
                "funding": {
                    "preferred_asset": "usdc",
                    "source_chain": "base"
                },
                "post_actions": []
            },
            "rationale": {
                "summary": "execute fast path",
                "main_risks": []
            },
            "execution_preferences": {
                "preview_only": false,
                "allow_fast_path": true
            }
        }),
    )
}
