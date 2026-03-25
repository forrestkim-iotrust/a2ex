use std::sync::{Arc, Mutex};

use a2ex_daemon::{
    AuthorizationResult, DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff,
};
use a2ex_ipc::{DAEMON_CONTROL_METHOD, JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord,
};
use tempfile::tempdir;

#[derive(Debug, Default)]
struct RecordingBridge {
    approvals: Mutex<Vec<ApprovalRequest>>,
}

#[derive(Debug, Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &ExecutionRequest) {}
}

impl RecordingBridge {
    fn approvals(&self) -> Vec<ApprovalRequest> {
        self.approvals.lock().expect("approvals lock").clone()
    }
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
}

#[tokio::test]
async fn signer_boundary_requires_ipc() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-1".to_owned(),
            execution_id: "exec-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let bridge = Arc::new(RecordingBridge::default());
    let signer = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        bridge.clone(),
        LocalPeerValidator::strict_local_only(),
    );
    let service = DaemonService::from_config_with_signer_bridge(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        signer,
    );

    let response: JsonRpcResponse<AuthorizationResult> = service
        .authorize_and_dispatch_to_signer(
            JsonRpcRequest::new(
                "req-invalid-peer",
                DAEMON_CONTROL_METHOD,
                ExecutionRequest {
                    action_id: "exec-1".to_owned(),
                    action_kind: "submit_intent".to_owned(),
                    notional_usd: 25,
                    reservation_id: "reservation-1".to_owned(),
                },
            ),
            LocalPeerIdentity::for_tests(false, false),
        )
        .await
        .expect("response builds");

    match response {
        JsonRpcResponse::Failure(failure) => {
            assert_eq!(failure.id, "req-invalid-peer");
            assert!(failure.error.message.contains("peer validation"));
        }
        JsonRpcResponse::Success(success) => {
            panic!(
                "expected signer boundary rejection, got {:?}",
                success.result
            )
        }
    }

    assert!(bridge.approvals().is_empty());
}

#[tokio::test]
async fn signer_boundary_dispatches_auditable_requests() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-1".to_owned(),
            execution_id: "exec-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let bridge = Arc::new(RecordingBridge::default());
    let signer = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        bridge.clone(),
        LocalPeerValidator::strict_local_only(),
    );
    let service = DaemonService::from_config_with_signer_bridge(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        signer,
    );

    let response: JsonRpcResponse<AuthorizationResult> = service
        .authorize_and_dispatch_to_signer(
            JsonRpcRequest::new(
                "req-valid-peer",
                DAEMON_CONTROL_METHOD,
                ExecutionRequest {
                    action_id: "exec-1".to_owned(),
                    action_kind: "submit_intent".to_owned(),
                    notional_usd: 25,
                    reservation_id: "reservation-1".to_owned(),
                },
            ),
            LocalPeerIdentity::for_tests(true, true),
        )
        .await
        .expect("response builds");

    assert!(matches!(response, JsonRpcResponse::Success(_)));

    let approvals = bridge.approvals();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].action_id, "exec-1");
    assert_eq!(approvals[0].reservation_id, "reservation-1");
    assert_eq!(approvals[0].action_kind, "submit_intent");
    assert_eq!(approvals[0].notional_usd, 25);
}
