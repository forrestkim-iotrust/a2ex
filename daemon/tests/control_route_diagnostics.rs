use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_ipc::{
    JsonRpcRequest, JsonRpcResponse, frame_transport, recv_json_message, send_json_message,
};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::tempdir;

const SUBMIT_INTENT_METHOD: &str = "platform.submitIntent";

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

struct RoundTripOutcome {
    _data_dir: tempfile::TempDir,
    state_db_path: std::path::PathBuf,
    response: Result<JsonRpcResponse<Value>, String>,
    server_result: Result<(), String>,
    signer_calls: usize,
}

async fn execute_request(method: &str, id: &str, params: Value) -> RoundTripOutcome {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    a2ex_daemon::load_runtime_state(&config)
        .await
        .expect("state repository opens");
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

    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server = {
        let service = service.clone();
        tokio::spawn(async move { service.serve_once(server_io).await })
    };
    let mut client = frame_transport(client_io);

    send_json_message(&mut client, &JsonRpcRequest::new(id, method, params))
        .await
        .expect("request sends");

    let response = recv_json_message(&mut client)
        .await
        .map_err(|error| error.to_string());
    let server_result = server
        .await
        .map_err(|error| format!("server task join failed: {error}"))
        .and_then(|result| result.map_err(|error| error.to_string()));

    RoundTripOutcome {
        _data_dir: data_dir,
        state_db_path: config.state_db_path(),
        response,
        server_result,
        signer_calls: signer.calls(),
    }
}

fn held_intent_params() -> Value {
    json!({
        "action_id": "intent-held-001",
        "action_kind": "submit_intent",
        "notional_usd": 250,
        "reservation_id": "reservation-intent-held-001",
        "request_id": "req-intent-held-001",
        "request_kind": "intent",
        "source_agent_id": "agent.s02.diagnostics",
        "submitted_at": "2026-03-12T00:04:00Z",
        "payload": {
            "intent_id": "intent-held-001",
            "intent_type": "open_exposure",
            "objective": {
                "domain": "prediction_market",
                "target_market": "fed-cut-2026",
                "side": "yes",
                "target_notional_usd": 250
            },
            "constraints": {
                "allowed_venues": ["polymarket"],
                "urgency": "normal",
                "max_slippage_bps": 40
            },
            "funding": {
                "preferred_asset": "USDC",
                "source_chain": "base"
            },
            "post_actions": []
        },
        "rationale": {
            "summary": "request should hold instead of executing"
        },
        "execution_preferences": {
            "preview_only": false,
            "allow_fast_path": true
        }
    })
}

fn invalid_intent_params() -> Value {
    json!({
        "action_id": "intent-invalid-001",
        "action_kind": "submit_intent",
        "notional_usd": 25,
        "reservation_id": "reservation-intent-invalid-001",
        "request_id": "req-intent-invalid-001",
        "request_kind": "intent",
        "source_agent_id": "agent.s02.diagnostics",
        "submitted_at": "2026-03-12T00:05:00Z",
        "payload": {
            "intent_id": "intent-invalid-001",
            "intent_type": "open_exposure",
            "objective": {
                "domain": "prediction_market",
                "target_market": "fed-cut-2026",
                "side": "yes"
            },
            "constraints": {
                "allowed_venues": ["polymarket"]
            },
            "funding": {
                "preferred_asset": "USDC"
            },
            "post_actions": []
        },
        "rationale": {
            "summary": "missing target_notional_usd should produce validation diagnostics"
        },
        "execution_preferences": {
            "preview_only": false,
            "allow_fast_path": true
        }
    })
}

#[tokio::test]
async fn held_intent_returns_persisted_route_diagnostics_without_execution_side_effects() {
    let outcome = execute_request(
        SUBMIT_INTENT_METHOD,
        "rpc-held-intent-diagnostics",
        held_intent_params(),
    )
    .await;

    let response = outcome
        .response
        .expect("held intake should return a structured response instead of closing the channel");
    assert!(
        outcome.server_result.is_ok(),
        "daemon should handle held intake without surfacing a transport error: {:?}",
        outcome.server_result
    );

    match response {
        JsonRpcResponse::Success(success) => {
            assert_eq!(success.result["request_id"], json!("req-intent-held-001"));
            assert_eq!(success.result["status"], json!("held"));
            assert_eq!(success.result["compile"]["status"], json!("validated"));
            assert_eq!(success.result["route"]["class"], json!("hold"));
            assert!(success.result["route"]["reasons"].is_array());
        }
        JsonRpcResponse::Failure(failure) => {
            panic!("expected held intake ack, got failure: {:?}", failure.error)
        }
    }

    let state = Connection::open(outcome.state_db_path).expect("state db opens");
    let request_status: (String, String) = state
        .query_row(
            "SELECT request_kind, status FROM agent_requests WHERE request_id = ?1",
            ["req-intent-held-001"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("held request row exists");
    assert_eq!(request_status.0, "intent");
    assert_eq!(request_status.1, "held");

    let route_payload: String = state
        .query_row(
            "SELECT payload_json FROM event_journal WHERE stream_type = 'agent_request' AND stream_id = ?1 AND event_type = 'agent_request_routed'",
            ["req-intent-held-001"],
            |row| row.get(0),
        )
        .expect("route diagnostics journal row exists");
    assert_eq!(
        serde_json::from_str::<Value>(&route_payload).expect("route payload parses")["route"]["class"],
        json!("hold")
    );

    let execution_rows: i64 = state
        .query_row("SELECT COUNT(*) FROM execution_states", [], |row| {
            row.get(0)
        })
        .expect("execution count query succeeds");
    let reservation_rows: i64 = state
        .query_row("SELECT COUNT(*) FROM capital_reservations", [], |row| {
            row.get(0)
        })
        .expect("reservation count query succeeds");
    assert_eq!(execution_rows, 0);
    assert_eq!(reservation_rows, 0);
    assert_eq!(outcome.signer_calls, 0);
}

#[tokio::test]
async fn invalid_control_intake_keeps_reservations_signer_and_execution_tables_untouched() {
    let outcome = execute_request(
        SUBMIT_INTENT_METHOD,
        "rpc-invalid-intent-diagnostics",
        invalid_intent_params(),
    )
    .await;

    assert!(
        outcome.response.is_err() || matches!(outcome.response, Ok(JsonRpcResponse::Failure(_))),
        "invalid intake should fail, not succeed: {:?}",
        outcome.server_result
    );

    let state = Connection::open(outcome.state_db_path).expect("state db opens");
    let reservation_rows: i64 = state
        .query_row("SELECT COUNT(*) FROM capital_reservations", [], |row| {
            row.get(0)
        })
        .expect("reservation count query succeeds");
    let execution_rows: i64 = state
        .query_row("SELECT COUNT(*) FROM execution_states", [], |row| {
            row.get(0)
        })
        .expect("execution count query succeeds");
    let runtime_rows: i64 = state
        .query_row("SELECT COUNT(*) FROM strategy_runtime_states", [], |row| {
            row.get(0)
        })
        .expect("runtime state count query succeeds");
    assert_eq!(reservation_rows, 0);
    assert_eq!(execution_rows, 0);
    assert_eq!(runtime_rows, 0);
    assert_eq!(outcome.signer_calls, 0);
}
