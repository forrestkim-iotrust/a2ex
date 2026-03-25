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
use serde_json::{Value, json};
use tempfile::tempdir;

const SUBMIT_INTENT_METHOD: &str = "platform.submitIntent";
const REGISTER_STRATEGY_METHOD: &str = "platform.registerStrategy";

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

async fn round_trip_request(
    method: &str,
    id: &str,
    params: Value,
) -> Result<(JsonRpcResponse<Value>, Arc<RecordingSigner>), String> {
    let data_dir = tempdir().map_err(|error| error.to_string())?;
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .map_err(|error| error.to_string())?;
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
        .map_err(|error| format!("request send failed: {error}"))?;

    let response = recv_json_message(&mut client)
        .await
        .map_err(|error| format!("response receive failed: {error}"))?;

    server
        .await
        .map_err(|error| format!("server task join failed: {error}"))?
        .map_err(|error| format!("server returned error: {error}"))?;

    Ok((response, signer))
}

fn submit_intent_params() -> Value {
    json!({
        "action_id": "intent-fast-001",
        "action_kind": "submit_intent",
        "notional_usd": 25,
        "reservation_id": "reservation-intent-fast-001",
        "request_id": "req-intent-fast-001",
        "request_kind": "intent",
        "source_agent_id": "agent.s02.contract",
        "submitted_at": "2026-03-12T00:00:00Z",
        "payload": {
            "intent_id": "intent-fast-001",
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
        "intent": {
            "intent_id": "intent-fast-001",
            "intent_type": "open_exposure"
        },
        "rationale": {
            "summary": "FAST-01 fast path intake contract"
        },
        "execution_preferences": {
            "preview_only": false,
            "allow_fast_path": true
        }
    })
}

fn register_strategy_params() -> Value {
    json!({
        "action_id": "strategy-runtime-001",
        "action_kind": "register_strategy",
        "notional_usd": 25,
        "reservation_id": "reservation-strategy-runtime-001",
        "request_id": "req-strategy-runtime-001",
        "request_kind": "strategy",
        "source_agent_id": "agent.s02.contract",
        "submitted_at": "2026-03-12T00:01:00Z",
        "payload": {
            "strategy_id": "strategy-runtime-001",
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
        "strategy": {
            "strategy_id": "strategy-runtime-001",
            "strategy_type": "stateful_hedge"
        },
        "rationale": {
            "summary": "AGNT-03 strategy registration contract"
        },
        "execution_preferences": {
            "preview_only": false,
            "allow_fast_path": false
        }
    })
}

#[tokio::test]
async fn submit_intent_returns_structured_fast_path_ack() {
    let (response, signer) = round_trip_request(
        SUBMIT_INTENT_METHOD,
        "rpc-submit-intent-fast-path",
        submit_intent_params(),
    )
    .await
    .expect(
        "daemon should acknowledge platform.submitIntent with structured route and compile fields",
    );

    match response {
        JsonRpcResponse::Success(success) => {
            assert_eq!(success.id, "rpc-submit-intent-fast-path");
            assert_eq!(success.result["request_id"], json!("req-intent-fast-001"));
            assert_eq!(success.result["request_kind"], json!("intent"));
            assert_eq!(success.result["intent_id"], json!("intent-fast-001"));
            assert_eq!(success.result["status"], json!("accepted"));
            assert_eq!(success.result["compile"]["status"], json!("validated"));
            assert_eq!(success.result["route"]["class"], json!("fast_path"));
            assert!(success.result["acknowledged_at"].is_string());
        }
        JsonRpcResponse::Failure(failure) => {
            panic!(
                "expected submitIntent success ack, got failure: {:?}",
                failure.error
            )
        }
    }

    assert_eq!(
        signer.calls(),
        0,
        "S02 intake must not reach signer handoff"
    );
}

#[tokio::test]
async fn register_strategy_returns_structured_stateful_runtime_ack() {
    let (response, signer) = round_trip_request(
        REGISTER_STRATEGY_METHOD,
        "rpc-register-strategy-stateful-runtime",
        register_strategy_params(),
    )
    .await
    .expect("daemon should acknowledge platform.registerStrategy with typed strategy registration fields");

    match response {
        JsonRpcResponse::Success(success) => {
            assert_eq!(success.id, "rpc-register-strategy-stateful-runtime");
            assert_eq!(
                success.result["request_id"],
                json!("req-strategy-runtime-001")
            );
            assert_eq!(success.result["request_kind"], json!("strategy"));
            assert_eq!(success.result["strategy_id"], json!("strategy-runtime-001"));
            assert_eq!(success.result["version"], json!(1));
            assert_eq!(success.result["status"], json!("accepted"));
            assert_eq!(success.result["compile"]["status"], json!("validated"));
            assert_eq!(success.result["route"]["class"], json!("stateful_runtime"));
            assert!(success.result["acknowledged_at"].is_string());
        }
        JsonRpcResponse::Failure(failure) => {
            panic!(
                "expected registerStrategy success ack, got failure: {:?}",
                failure.error
            )
        }
    }

    assert_eq!(
        signer.calls(),
        0,
        "S02 strategy intake must remain side-effect-free"
    );
}
