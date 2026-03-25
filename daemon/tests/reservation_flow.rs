use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use a2ex_daemon::{
    AuthorizationResult, DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff,
};
use a2ex_ipc::{
    DAEMON_CONTROL_METHOD, JsonRpcRequest, JsonRpcResponse, frame_transport, recv_json_message,
    send_json_message,
};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use rusqlite::Connection;
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
async fn reservation_required_for_execution() {
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

    let request = JsonRpcRequest::new(
        "req-missing-reservation",
        DAEMON_CONTROL_METHOD,
        ExecutionRequest {
            action_id: "exec-without-reservation".to_owned(),
            action_kind: "submit_intent".to_owned(),
            notional_usd: 25,
            reservation_id: "reservation-missing".to_owned(),
        },
    );

    send_json_message(&mut client, &request)
        .await
        .expect("request sent");
    let response: JsonRpcResponse<AuthorizationResult> = recv_json_message(&mut client)
        .await
        .expect("response received");

    match response {
        JsonRpcResponse::Failure(failure) => {
            assert_eq!(failure.id, "req-missing-reservation");
            assert!(failure.error.message.contains("reservation"));
        }
        JsonRpcResponse::Success(success) => {
            panic!("expected reservation failure, got {:?}", success.result)
        }
    }

    server
        .await
        .expect("server task joins")
        .expect("server handles request");
    assert_eq!(signer.calls(), 0);
}

#[tokio::test]
async fn successful_execution_records_hold_consume_and_release() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let held = reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-1".to_owned(),
            execution_id: "exec-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold succeeds");
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

    let request = JsonRpcRequest::new(
        "req-held-reservation",
        DAEMON_CONTROL_METHOD,
        ExecutionRequest {
            action_id: "exec-1".to_owned(),
            action_kind: "submit_intent".to_owned(),
            notional_usd: 25,
            reservation_id: held.reservation_id.clone(),
        },
    );

    send_json_message(&mut client, &request)
        .await
        .expect("request sent");
    let response: JsonRpcResponse<AuthorizationResult> = recv_json_message(&mut client)
        .await
        .expect("response received");

    assert!(matches!(response, JsonRpcResponse::Success(_)));
    server
        .await
        .expect("server task joins")
        .expect("server handles request");

    let db = Connection::open(config.state_db_path()).expect("state db opens");
    let transitions: Vec<(String, String)> = db
        .prepare(
            "SELECT event_type, payload_json
             FROM event_journal
             WHERE stream_id = ?1
             ORDER BY rowid",
        )
        .expect("statement prepares")
        .query_map([held.reservation_id.as_str()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("query executes")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows collect");

    assert_eq!(transitions.len(), 3);
    assert_eq!(
        transitions
            .iter()
            .map(|(event_type, _)| event_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "reservation_held",
            "reservation_consumed",
            "reservation_released"
        ],
    );
}
