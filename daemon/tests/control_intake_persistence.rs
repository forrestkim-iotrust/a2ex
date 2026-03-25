use std::sync::Arc;

use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_ipc::{
    JsonRpcRequest, JsonRpcResponse, frame_transport, recv_json_message, send_json_message,
};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use tempfile::tempdir;

const SUBMIT_INTENT_METHOD: &str = "platform.submitIntent";
const REGISTER_STRATEGY_METHOD: &str = "platform.registerStrategy";

#[derive(Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &ExecutionRequest) {}
}

async fn send_request_and_wait(
    method: &str,
    id: &str,
    params: Value,
    config: &DaemonConfig,
) -> Result<JsonRpcResponse<Value>, String> {
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .map_err(|error| error.to_string())?;
    let service = Arc::new(DaemonService::from_config(
        config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
    ));

    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server = {
        let service = service.clone();
        tokio::spawn(async move { service.serve_once(server_io).await })
    };
    let mut client = frame_transport(client_io);

    send_json_message(&mut client, &JsonRpcRequest::new(id, method, params))
        .await
        .map_err(|error| format!("request send failed: {error}"))?;

    let response = recv_json_message(&mut client)
        .await
        .map_err(|error| format!("response receive failed: {error}"))?;

    server
        .await
        .map_err(|error| format!("server task join failed: {error}"))?
        .map_err(|error| format!("server returned error: {error}"))?;

    Ok(response)
}

fn submit_intent_params() -> Value {
    json!({
        "action_id": "intent-persist-001",
        "action_kind": "submit_intent",
        "notional_usd": 25,
        "reservation_id": "reservation-intent-persist-001",
        "request_id": "req-intent-persist-001",
        "request_kind": "intent",
        "source_agent_id": "agent.s02.persistence",
        "submitted_at": "2026-03-12T00:02:00Z",
        "payload": {
            "intent_id": "intent-persist-001",
            "intent_type": "open_exposure",
            "objective": {
                "domain": "prediction_market",
                "target_market": "fed-cut-2026",
                "side": "yes",
                "target_notional_usd": 25
            },
            "constraints": {
                "allowed_venues": ["polymarket"],
                "urgency": "immediate",
                "max_slippage_bps": 40
            },
            "funding": {
                "preferred_asset": "USDC",
                "source_chain": "base"
            },
            "post_actions": []
        },
        "rationale": {
            "summary": "persist canonical request envelope"
        },
        "execution_preferences": {
            "preview_only": false,
            "allow_fast_path": true
        }
    })
}

fn register_strategy_params() -> Value {
    json!({
        "action_id": "strategy-persist-001",
        "action_kind": "register_strategy",
        "notional_usd": 25,
        "reservation_id": "reservation-strategy-persist-001",
        "request_id": "req-strategy-persist-001",
        "request_kind": "strategy",
        "source_agent_id": "agent.s02.persistence",
        "submitted_at": "2026-03-12T00:03:00Z",
        "payload": {
            "strategy_id": "strategy-persist-001",
            "strategy_type": "stateful_hedge",
            "watchers": [
                {
                    "watcher_type": "venue_position",
                    "source": "hyperliquid",
                    "instrument": "BTC-PERP"
                }
            ],
            "trigger_rules": [
                {
                    "trigger_type": "drift_threshold",
                    "metric": "delta_exposure_pct",
                    "operator": ">",
                    "value": 0.02,
                    "cooldown_sec": 10
                }
            ],
            "calculation_model": {
                "model_type": "delta_neutral_lp",
                "inputs": ["position_delta"]
            },
            "action_templates": [
                {
                    "action_type": "adjust_hedge",
                    "venue": "hyperliquid",
                    "instrument": "BTC-PERP",
                    "target": "delta_neutral"
                }
            ],
            "constraints": {
                "min_order_usd": 100,
                "max_slippage_bps": 40,
                "max_rebalances_per_hour": 60
            },
            "unwind_rules": [{ "condition": "manual_stop" }]
        },
        "rationale": {
            "summary": "persist strategy and seed idle runtime state"
        },
        "execution_preferences": {
            "preview_only": false,
            "allow_fast_path": false
        }
    })
}

#[tokio::test]
async fn submit_intent_persists_raw_request_and_normalized_intent_rows() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());

    let response = send_request_and_wait(
        SUBMIT_INTENT_METHOD,
        "rpc-intent-persistence",
        submit_intent_params(),
        &config,
    )
    .await
    .expect("submitIntent should persist canonical request state before returning");

    assert!(matches!(response, JsonRpcResponse::Success(_)));

    let state = Connection::open(config.state_db_path()).expect("state db opens");
    let request_row: (String, String, String, String, String, String) = state
        .query_row(
            "SELECT request_kind, source_agent_id, rationale_json, execution_prefs_json, payload_json, created_at
             FROM agent_requests
             WHERE request_id = ?1",
            ["req-intent-persist-001"],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("agent request row exists");
    assert_eq!(request_row.0, "intent");
    assert_eq!(request_row.1, "agent.s02.persistence");
    assert!(
        serde_json::from_str::<Value>(&request_row.2).expect("rationale json parses")["summary"]
            == json!("persist canonical request envelope")
    );
    assert!(
        serde_json::from_str::<Value>(&request_row.3).expect("execution prefs json parses")["allow_fast_path"]
            == json!(true)
    );
    assert!(
        serde_json::from_str::<Value>(&request_row.4).expect("payload json parses")["intent_id"]
            == json!("intent-persist-001")
    );
    assert!(!request_row.5.is_empty());

    let intent_row: (String, String, String) = state
        .query_row(
            "SELECT request_id, status, intent_type FROM intents WHERE intent_id = ?1",
            ["intent-persist-001"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("normalized intent row exists");
    assert_eq!(intent_row.0, "req-intent-persist-001");
    assert_eq!(intent_row.1, "validated");
    assert_eq!(intent_row.2, "open_exposure");

    let journal_events: Vec<String> = state
        .prepare(
            "SELECT event_type FROM event_journal WHERE stream_type = 'agent_request' AND stream_id = ?1 ORDER BY created_at, event_id",
        )
        .expect("event journal statement prepares")
        .query_map(["req-intent-persist-001"], |row| row.get(0))
        .expect("event journal query runs")
        .collect::<Result<Vec<String>, _>>()
        .expect("event journal rows collect");
    assert_eq!(
        journal_events,
        vec![
            "agent_request_received",
            "agent_request_compiled",
            "agent_request_routed",
        ]
    );
}

#[tokio::test]
async fn register_strategy_persists_versions_and_idle_runtime_state_across_restart() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());

    let response = send_request_and_wait(
        REGISTER_STRATEGY_METHOD,
        "rpc-strategy-persistence",
        register_strategy_params(),
        &config,
    )
    .await
    .expect("registerStrategy should persist strategy records before returning");

    assert!(matches!(response, JsonRpcResponse::Success(_)));

    let first = Connection::open(config.state_db_path()).expect("state db opens");
    let strategy_row: (String, i64) = first
        .query_row(
            "SELECT request_id, current_version FROM strategies WHERE strategy_id = ?1",
            ["strategy-persist-001"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("strategy row exists");
    assert_eq!(strategy_row.0, "req-strategy-persist-001");
    assert_eq!(strategy_row.1, 1);

    let version_payload: String = first
        .query_row(
            "SELECT spec_json FROM strategy_versions WHERE strategy_id = ?1 AND version = 1",
            ["strategy-persist-001"],
            |row| row.get(0),
        )
        .expect("strategy version row exists");
    assert_eq!(
        serde_json::from_str::<Value>(&version_payload).expect("version spec json parses")["strategy_type"],
        json!("stateful_hedge")
    );

    let seeded_runtime_state: String = first
        .query_row(
            "SELECT runtime_state FROM strategy_runtime_states WHERE strategy_id = ?1",
            ["strategy-persist-001"],
            |row| row.get(0),
        )
        .expect("strategy runtime state row exists");
    assert_eq!(seeded_runtime_state, "idle");
    drop(first);

    let second = Connection::open(config.state_db_path()).expect("state db reopens after restart");
    let restarted_runtime_state: String = second
        .query_row(
            "SELECT runtime_state FROM strategy_runtime_states WHERE strategy_id = ?1",
            ["strategy-persist-001"],
            |row| row.get(0),
        )
        .expect("strategy runtime state still exists after restart");
    assert_eq!(restarted_runtime_state, "idle");

    let strategy_event_count: i64 = second
        .query_row(
            "SELECT COUNT(*) FROM event_journal WHERE stream_type = 'agent_request' AND stream_id = ?1 AND event_type = 'agent_request_routed'",
            params!["req-strategy-persist-001"],
            |row| row.get(0),
        )
        .expect("strategy route journal count loads");
    assert_eq!(strategy_event_count, 1);
}
