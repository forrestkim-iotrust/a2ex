use std::sync::Arc;

use a2ex_daemon::{
    DaemonConfig, DaemonService, IntentSubmissionReceipt, SignerHandoff,
    StrategyRegistrationReceipt,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_state::StateRepository;
use serde_json::json;
use tempfile::tempdir;

#[derive(Default)]
struct RecordingSigner;

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {}
}

#[tokio::test]
async fn decision_gateway_routes_compiled_requests_consistently() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(RecordingSigner),
    );

    let fast_response: JsonRpcResponse<IntentSubmissionReceipt> = service
        .submit_intent(intent_request(
            "req-intent-fast",
            "intent-fast",
            vec!["Polymarket"],
            0,
            vec![],
            false,
        ))
        .await
        .expect("fast intent responds");
    assert!(matches!(fast_response, JsonRpcResponse::Success(_)));

    let planned_response: JsonRpcResponse<IntentSubmissionReceipt> = service
        .submit_intent(intent_request(
            "req-intent-planned",
            "intent-planned",
            vec!["Polymarket", "Kalshi"],
            4000,
            vec![json!({
                "action_type": "HEDGE",
                "venue": "Hyperliquid"
            })],
            false,
        ))
        .await
        .expect("planned intent responds");
    assert!(matches!(planned_response, JsonRpcResponse::Success(_)));

    let strategy_response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request("req-strategy-runtime", false))
        .await
        .expect("strategy registration responds");
    assert!(matches!(strategy_response, JsonRpcResponse::Success(_)));

    let hold_response: JsonRpcResponse<IntentSubmissionReceipt> = service
        .submit_intent(intent_request(
            "req-intent-hold",
            "intent-hold",
            vec!["Polymarket"],
            0,
            vec![],
            true,
        ))
        .await
        .expect("hold intent responds");
    assert!(matches!(hold_response, JsonRpcResponse::Success(_)));

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("state repository opens");

    let fast_route = repository
        .load_route_decision("req-intent-fast")
        .await
        .expect("fast route loads")
        .expect("fast route persisted");
    assert_eq!(fast_route.route.route, a2ex_control::RouteTarget::FastPath);

    let planned_route = repository
        .load_route_decision("req-intent-planned")
        .await
        .expect("planned route loads")
        .expect("planned route persisted");
    assert_eq!(
        planned_route.route.route,
        a2ex_control::RouteTarget::PlannedExecution
    );

    let strategy_route = repository
        .load_route_decision("req-strategy-runtime")
        .await
        .expect("strategy route loads")
        .expect("strategy route persisted");
    assert_eq!(
        strategy_route.route.route,
        a2ex_control::RouteTarget::StatefulRuntime
    );

    let hold_route = repository
        .load_route_decision("req-intent-hold")
        .await
        .expect("hold route loads")
        .expect("hold route persisted");
    assert_eq!(hold_route.route.route, a2ex_control::RouteTarget::Hold);
    assert_eq!(
        hold_route.route.hold_reason.as_deref(),
        Some("preview_only")
    );

    let journal = repository.load_journal().await.expect("journal loads");
    assert!(journal.iter().any(|entry| {
        entry.stream_type == "agent_request"
            && entry.stream_id == "req-intent-fast"
            && entry.event_type == "route_decision_recorded"
            && entry.payload_json.contains("fast_path")
    }));
}

fn intent_request(
    request_id: &str,
    intent_id: &str,
    allowed_venues: Vec<&str>,
    hedge_ratio_bps: u64,
    post_actions: Vec<serde_json::Value>,
    preview_only: bool,
) -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        request_id,
        "daemon.submitIntent",
        json!({
            "request_id": request_id,
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_id": intent_id,
                "intent_type": "Open_Exposure",
                "objective": {
                    "domain": "Prediction_Market",
                    "target_market": "US-Election-2028",
                    "side": "YES",
                    "target_notional_usd": 3000
                },
                "constraints": {
                    "allowed_venues": allowed_venues,
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "high",
                    "hedge_ratio_bps": hedge_ratio_bps
                },
                "funding": {
                    "preferred_asset": "usdc",
                    "source_chain": "Base"
                },
                "post_actions": post_actions
            },
            "rationale": {
                "summary": "Opportunity remains positive after costs.",
                "main_risks": ["spread compression"]
            },
            "execution_preferences": {
                "preview_only": preview_only,
                "allow_fast_path": true
            }
        }),
    )
}

fn strategy_request(request_id: &str, preview_only: bool) -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        request_id,
        "daemon.registerStrategy",
        json!({
            "request_id": request_id,
            "request_kind": "strategy",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "strategy_id": "strategy-lp-1",
                "strategy_type": "stateful_hedge",
                "watchers": [
                    {
                        "watcher_type": "lp_position",
                        "source": "uniswap_v2",
                        "chain": "arbitrum",
                        "target": "TOKEN/USDT"
                    }
                ],
                "trigger_rules": [
                    {
                        "trigger_type": "drift_threshold",
                        "metric": "delta_exposure_pct",
                        "operator": ">",
                        "value": "0.02",
                        "cooldown_sec": 10
                    }
                ],
                "calculation_model": {
                    "model_type": "delta_neutral_lp",
                    "inputs": ["lp_token_balance", "pool_reserves", "current_hedge_position"]
                },
                "action_templates": [
                    {
                        "action_type": "adjust_hedge",
                        "venue": "hyperliquid",
                        "instrument": "TOKEN-PERP",
                        "target": "delta_neutral"
                    }
                ],
                "constraints": {
                    "min_order_usd": 100,
                    "max_slippage_bps": 40,
                    "max_rebalances_per_hour": 60
                },
                "unwind_rules": [
                    { "condition": "manual_stop" }
                ]
            },
            "rationale": {
                "summary": "Keep LP exposure delta neutral.",
                "main_risks": ["watcher lag"]
            },
            "execution_preferences": {
                "preview_only": preview_only,
                "allow_fast_path": false,
                "client_request_label": "lp-runtime"
            }
        }),
    )
}
