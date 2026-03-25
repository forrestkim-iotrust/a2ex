use std::sync::Arc;

use a2ex_daemon::{
    DaemonConfig, DaemonService, IntentSubmissionReceipt, SignerHandoff,
    StrategyRegistrationReceipt,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use serde_json::json;
use tempfile::tempdir;

#[derive(Default)]
struct RecordingSigner;

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {}
}

#[tokio::test]
async fn compiler_normalizes_agent_requests_into_ir() {
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

    let intent_response: JsonRpcResponse<IntentSubmissionReceipt> = service
        .submit_intent(intent_request())
        .await
        .expect("intent submission responds");
    assert!(matches!(intent_response, JsonRpcResponse::Success(_)));

    let strategy_response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request("adjust_hedge", "0.02", None))
        .await
        .expect("strategy registration responds");
    assert!(matches!(strategy_response, JsonRpcResponse::Success(_)));

    let compiled_intents = service.compiled_intents();
    assert_eq!(compiled_intents.len(), 1);
    assert_eq!(compiled_intents[0].intent_id, "intent-1");
    assert_eq!(
        compiled_intents[0].constraints.allowed_venues,
        vec!["kalshi", "polymarket"]
    );
    assert_eq!(compiled_intents[0].constraints.hedge_ratio_bps, 4000);
    assert_eq!(compiled_intents[0].funding.preferred_asset, "USDC");
    assert_eq!(compiled_intents[0].post_actions[0].action_type, "hedge");
    assert_eq!(
        compiled_intents[0].audit.rationale_summary,
        "Opportunity remains positive after costs."
    );

    let compiled_strategies = service.compiled_strategies();
    assert_eq!(compiled_strategies.len(), 1);
    assert_eq!(compiled_strategies[0].strategy_id, "strategy-lp-1");
    assert_eq!(compiled_strategies[0].trigger_rules[0].threshold, 0.02);
    assert_eq!(
        compiled_strategies[0].constraints.max_rebalances_per_hour,
        60
    );
    assert_eq!(
        compiled_strategies[0].audit.rationale_summary,
        "Keep LP exposure delta neutral."
    );

    let invalid_response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request("call_contract", "0.02", Some(0)))
        .await
        .expect("invalid strategy responds");

    match invalid_response {
        JsonRpcResponse::Failure(failure) => {
            assert_eq!(failure.id, "req-strategy-1");
            assert!(failure.error.message.contains("unsupported_action_type"));
            assert!(failure.error.message.contains("invalid_constraint"));
        }
        JsonRpcResponse::Success(success) => {
            panic!("expected compiler failure, got {:?}", success.result)
        }
    }

    assert_eq!(
        service.compiled_strategies().len(),
        1,
        "failed compilation should not add IR"
    );
}

fn intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-intent-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-intent-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_id": "Intent-1",
                "intent_type": "Open_Exposure",
                "objective": {
                    "domain": "Prediction_Market",
                    "target_market": "US-Election-2028",
                    "side": "YES",
                    "target_notional_usd": 3000
                },
                "constraints": {
                    "allowed_venues": ["Polymarket", " kalshi "],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "normal",
                    "hedge_ratio_bps": 4000
                },
                "funding": {
                    "preferred_asset": "usdc",
                    "source_chain": "Base"
                },
                "post_actions": [
                    {
                        "action_type": "HEDGE",
                        "venue": "Hyperliquid"
                    }
                ]
            },
            "rationale": {
                "summary": "Opportunity remains positive after costs.",
                "main_risks": ["spread compression"]
            },
            "execution_preferences": {
                "preview_only": false,
                "allow_fast_path": true
            }
        }),
    )
}

fn strategy_request(
    action_type: &str,
    trigger_value: &str,
    max_rebalances_per_hour: Option<u64>,
) -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-strategy-1",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-1",
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
                        "value": trigger_value,
                        "cooldown_sec": 10
                    }
                ],
                "calculation_model": {
                    "model_type": "delta_neutral_lp",
                    "inputs": ["lp_token_balance", "pool_reserves", "current_hedge_position"]
                },
                "action_templates": [
                    {
                        "action_type": action_type,
                        "venue": "hyperliquid",
                        "instrument": "TOKEN-PERP",
                        "target": "delta_neutral"
                    }
                ],
                "constraints": {
                    "min_order_usd": 100,
                    "max_slippage_bps": 40,
                    "max_rebalances_per_hour": max_rebalances_per_hour
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
                "preview_only": false,
                "allow_fast_path": false,
                "client_request_label": "lp-runtime"
            }
        }),
    )
}
