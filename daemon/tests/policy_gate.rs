use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use a2ex_daemon::{
    AuthorizationResult, AuthorizationVerdict, DaemonConfig, DaemonService, ExecutionRequest,
    SignerHandoff,
};
use a2ex_ipc::{
    DAEMON_CONTROL_METHOD, JsonRpcRequest, JsonRpcResponse, frame_transport, recv_json_message,
    send_json_message,
};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use tempfile::tempdir;

#[derive(Default)]
struct RecordingSigner {
    calls: AtomicUsize,
}

impl RecordingSigner {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, _request: &ExecutionRequest) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn policy_gate_blocks_before_signer() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let signer = Arc::new(RecordingSigner::default());
    let service = Arc::new(DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        signer.clone(),
    ));
    let (client_io, server_io) = tokio::io::duplex(4096);
    let server = {
        let service = service.clone();
        tokio::spawn(async move { service.serve_once(server_io).await })
    };
    let mut client = frame_transport(client_io);

    let blocked = JsonRpcRequest::new(
        "req-blocked",
        DAEMON_CONTROL_METHOD,
        ExecutionRequest {
            action_id: "exec-blocked".to_owned(),
            action_kind: "blocked_by_policy".to_owned(),
            notional_usd: 250,
            reservation_id: "reservation-blocked".to_owned(),
        },
    );

    send_json_message(&mut client, &blocked)
        .await
        .expect("blocked request sent");
    let response: JsonRpcResponse<AuthorizationResult> = recv_json_message(&mut client)
        .await
        .expect("blocked response received");

    match response {
        JsonRpcResponse::Failure(failure) => {
            assert_eq!(failure.id, "req-blocked");
            assert!(failure.error.message.contains("blocked"));
        }
        JsonRpcResponse::Success(success) => {
            panic!("expected policy rejection, got {:?}", success.result)
        }
    }

    server
        .await
        .expect("server task joins")
        .expect("server handles blocked request");
    assert_eq!(signer.calls(), 0);
}

#[tokio::test]
async fn allowed_action_returns_pre_reservation_authorization() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-allowed".to_owned(),
            execution_id: "exec-allowed".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let signer = Arc::new(RecordingSigner::default());
    let service = Arc::new(DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        signer.clone(),
    ));
    let (client_io, server_io) = tokio::io::duplex(4096);
    let server = {
        let service = service.clone();
        tokio::spawn(async move { service.serve_once(server_io).await })
    };
    let mut client = frame_transport(client_io);

    let allowed = JsonRpcRequest::new(
        "req-allowed",
        DAEMON_CONTROL_METHOD,
        ExecutionRequest {
            action_id: "exec-allowed".to_owned(),
            action_kind: "submit_intent".to_owned(),
            notional_usd: 25,
            reservation_id: "reservation-allowed".to_owned(),
        },
    );

    send_json_message(&mut client, &allowed)
        .await
        .expect("allowed request sent");
    let response: JsonRpcResponse<AuthorizationResult> = recv_json_message(&mut client)
        .await
        .expect("allowed response received");

    match response {
        JsonRpcResponse::Success(success) => {
            assert_eq!(success.id, "req-allowed");
            assert_eq!(success.result.action_id, "exec-allowed");
            assert_eq!(success.result.verdict, AuthorizationVerdict::Allow);
        }
        JsonRpcResponse::Failure(failure) => panic!("expected allow, got {:?}", failure.error),
    }

    server
        .await
        .expect("server task joins")
        .expect("server handles allowed request");
    assert_eq!(signer.calls(), 0);
}
