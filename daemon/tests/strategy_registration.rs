use std::sync::Arc;

use a2ex_control::{
    ActionTemplate, AgentRequestEnvelope, AgentRequestKind, CalculationModel, ExecutionPreferences,
    RationaleSummary, Strategy, StrategyConstraints, TriggerRule, UnwindRule, WatcherSpec,
};
use a2ex_daemon::{DaemonConfig, DaemonService, SignerHandoff, StrategyRegistrationReceipt};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_state::StateRepository;
use serde_json::json;
use tempfile::tempdir;

#[derive(Default)]
struct RecordingSigner {
    handoff_count: std::sync::atomic::AtomicUsize,
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {
        self.handoff_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn strategy_registration_persists_structured_watchers_and_actions() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let signer = Arc::new(RecordingSigner::default());
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        signer.clone(),
    );

    let request = strategy_registration_request();

    let response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(request)
        .await
        .expect("register strategy succeeds");

    match response {
        JsonRpcResponse::Success(success) => {
            assert_eq!(success.id, "req-strategy-1");
            assert_eq!(success.result.request_id, "req-strategy-1");
            assert_eq!(success.result.strategy_id, "strategy-lp-1");
        }
        JsonRpcResponse::Failure(failure) => panic!("expected success, got {:?}", failure.error),
    }

    assert_eq!(
        service.recorded_strategies(),
        vec![expected_strategy_envelope()]
    );

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("state repository opens");
    let stored = repository
        .load_strategy_registration("strategy-lp-1")
        .await
        .expect("strategy query succeeds")
        .expect("strategy persisted");

    assert_eq!(stored.request_id, "req-strategy-1");
    assert_eq!(stored.source_agent_id, "agent-main");
    assert_eq!(stored.strategy_type, "stateful_hedge");
    assert_eq!(stored.watchers.len(), 2);
    assert_eq!(stored.trigger_rules.len(), 1);
    assert_eq!(stored.action_templates.len(), 1);

    let journal = repository.load_journal().await.expect("journal loads");
    let strategy_event = journal
        .iter()
        .find(|entry| {
            entry.event_type == "strategy_registered" && entry.stream_id == "strategy-lp-1"
        })
        .expect("strategy journal entry exists");
    assert!(strategy_event.payload_json.contains("lp_position"));
    assert!(strategy_event.payload_json.contains("adjust_hedge"));
}

#[tokio::test]
async fn strategy_registration_seeds_runtime_state_without_starting_watchers() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let signer = Arc::new(RecordingSigner::default());
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        signer.clone(),
    );

    let response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_registration_request())
        .await
        .expect("register strategy succeeds");

    match response {
        JsonRpcResponse::Success(success) => {
            assert_eq!(success.result.strategy_id, "strategy-lp-1");
        }
        JsonRpcResponse::Failure(failure) => panic!("expected success, got {:?}", failure.error),
    }

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("state repository opens");
    let snapshot = repository.load_snapshot().await.expect("snapshot loads");
    assert_eq!(
        snapshot.executions.len(),
        0,
        "registration should not create execution state"
    );
    assert_eq!(
        snapshot.reconciliations.len(),
        0,
        "registration should not create reconciliation state"
    );
    assert_eq!(
        snapshot.strategies.len(),
        1,
        "registration should seed one runtime state"
    );
    assert_eq!(snapshot.strategies[0].strategy_id, "strategy-lp-1");
    assert_eq!(snapshot.strategies[0].runtime_state, "idle");

    let last_transition_at = repository
        .last_strategy_transition_at("strategy-lp-1")
        .await
        .expect("strategy state query succeeds")
        .expect("runtime state persisted");
    assert_eq!(last_transition_at, "2026-03-11T00:00:00Z");

    assert_eq!(
        signer
            .handoff_count
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "strategy registration should not hand off to signer"
    );
}

fn strategy_registration_request() -> JsonRpcRequest<serde_json::Value> {
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
                    },
                    {
                        "watcher_type": "venue_position",
                        "source": "hyperliquid",
                        "target": "TOKEN-PERP"
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
                "preview_only": false,
                "allow_fast_path": false,
                "client_request_label": "lp-runtime"
            }
        }),
    )
}

fn expected_strategy_envelope() -> AgentRequestEnvelope<Strategy> {
    AgentRequestEnvelope {
        request_id: "req-strategy-1".to_owned(),
        request_kind: AgentRequestKind::Strategy,
        source_agent_id: "agent-main".to_owned(),
        submitted_at: "2026-03-11T00:00:00Z".to_owned(),
        payload: Strategy {
            strategy_id: "strategy-lp-1".to_owned(),
            strategy_type: "stateful_hedge".to_owned(),
            watchers: vec![
                WatcherSpec {
                    watcher_type: "lp_position".to_owned(),
                    source: "uniswap_v2".to_owned(),
                    chain: Some("arbitrum".to_owned()),
                    target: Some("TOKEN/USDT".to_owned()),
                },
                WatcherSpec {
                    watcher_type: "venue_position".to_owned(),
                    source: "hyperliquid".to_owned(),
                    chain: None,
                    target: Some("TOKEN-PERP".to_owned()),
                },
            ],
            trigger_rules: vec![TriggerRule {
                trigger_type: "drift_threshold".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                operator: ">".to_owned(),
                value: "0.02".to_owned(),
                cooldown_sec: Some(10),
            }],
            calculation_model: CalculationModel {
                model_type: "delta_neutral_lp".to_owned(),
                inputs: vec![
                    "lp_token_balance".to_owned(),
                    "pool_reserves".to_owned(),
                    "current_hedge_position".to_owned(),
                ],
            },
            action_templates: vec![ActionTemplate {
                action_type: "adjust_hedge".to_owned(),
                venue: "hyperliquid".to_owned(),
                instrument: Some("TOKEN-PERP".to_owned()),
                target: Some("delta_neutral".to_owned()),
            }],
            constraints: StrategyConstraints {
                min_order_usd: Some(100),
                max_slippage_bps: 40,
                max_rebalances_per_hour: Some(60),
            },
            unwind_rules: vec![UnwindRule {
                condition: "manual_stop".to_owned(),
            }],
        },
        rationale: RationaleSummary {
            summary: "Keep LP exposure delta neutral.".to_owned(),
            main_risks: vec!["watcher lag".to_owned()],
        },
        execution_preferences: ExecutionPreferences {
            preview_only: false,
            allow_fast_path: false,
            client_request_label: Some("lp-runtime".to_owned()),
        },
    }
}
