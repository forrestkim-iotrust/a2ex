use std::sync::Arc;

use a2ex_daemon::{
    DaemonConfig, DaemonService, SignerHandoff, load_event_journal, load_runtime_state,
};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_ipc::JsonRpcRequest;
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignedTx, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord, TxSignRequest,
};
use tempfile::tempdir;

#[derive(Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {}
}

#[derive(Default)]
struct SigningBridge;

#[async_trait::async_trait]
impl SignerBridge for SigningBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }

    async fn sign_transaction(&self, req: TxSignRequest) -> Result<SignedTx, SignerBridgeError> {
        Ok(SignedTx { bytes: req.payload })
    }
}

#[tokio::test]
async fn fast_path_persists_execution_lifecycle() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-state".to_owned(),
            execution_id: "exec-state".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let signer_bridge = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        Arc::new(SigningBridge),
        LocalPeerValidator::strict_local_only(),
    );
    let service = DaemonService::from_config_with_fast_path(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        signer_bridge,
        SimulatedEvmAdapter {
            block_number: 21,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
    );

    service
        .submit_intent(fast_intent_request())
        .await
        .expect("intent persists");
    let prepared = service
        .prepare_fast_path_action("req-fast-state", "reservation-state")
        .await
        .expect("prepared action");
    service
        .execute_fast_path_action(&prepared, LocalPeerIdentity::for_tests(true, true))
        .await
        .expect("execution succeeds");

    let snapshot = load_runtime_state(&config).await.expect("snapshot loads");
    assert_eq!(snapshot.executions.len(), 1);
    assert_eq!(snapshot.executions[0].execution_id, prepared.action_id);
    assert_eq!(snapshot.executions[0].status, "confirmed");

    let journal = load_event_journal(&config).await.expect("journal loads");
    assert!(journal.iter().any(|entry| {
        entry.stream_id == snapshot.executions[0].execution_id
            && entry.payload_json.contains("confirmed")
            && entry.event_type == "execution_state_changed"
    }));
}

fn fast_intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-fast-state",
        "daemon.submitIntent",
        serde_json::json!({
            "request_id": "req-fast-state",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_id": "intent-fast-state",
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "market-state",
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
                "summary": "persist lifecycle",
                "main_risks": []
            },
            "execution_preferences": {
                "preview_only": false,
                "allow_fast_path": true
            }
        }),
    )
}
