use std::{sync::Arc, time::Duration};

use a2ex_compiler::compile_strategy;
use a2ex_control::{
    ActionTemplate, AgentRequestEnvelope, AgentRequestKind, CalculationModel, ExecutionPreferences,
    RationaleSummary, Strategy, StrategyConstraints, TriggerRule, UnwindRule, WatcherSpec,
};
use a2ex_daemon::{
    DaemonConfig, DaemonService, SignerHandoff, StrategyRegistrationReceipt,
    spawn_local_daemon_with_service,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_state::StateRepository;
use a2ex_strategy_runtime::{
    RuntimeEvent, RuntimeWatcherState, StrategyRuntimeSnapshot, StrategySupervisor,
    supervisor_interval,
};
use serde_json::json;
use tempfile::tempdir;
use tokio::{
    sync::mpsc,
    time::{MissedTickBehavior, sleep},
};

#[derive(Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {}
}

#[tokio::test]
async fn strategy_runtime_watcher_samples_persist_cursors() {
    let (_data_dir, config, service) = setup_service().await;
    register_strategy(&service).await;

    service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "lp-position".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-11T00:00:10Z".to_owned(),
            }],
            "2026-03-11T00:00:10Z",
        )
        .await
        .expect("strategy evaluates");

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("snapshot exists");

    assert_eq!(snapshot.last_event_id.as_deref(), Some("evt-1"));
    assert_eq!(snapshot.watcher_states.len(), 1);
    assert_eq!(snapshot.watcher_states[0].cursor, "evt-1");
}

#[tokio::test]
async fn strategy_runtime_supervisor_evaluates_from_watcher_events_and_ticks() {
    let (supervisor, mut outputs) = runtime_supervisor().await;

    supervisor
        .send(RuntimeEvent::WatcherSample(RuntimeWatcherState {
            watcher_key: "lp-position".to_owned(),
            metric: "delta_exposure_pct".to_owned(),
            value: 0.031,
            cursor: "evt-1".to_owned(),
            sampled_at: "2026-03-11T00:00:10Z".to_owned(),
        }))
        .await
        .expect("watcher event sends");

    let first = outputs.recv().await.expect("watcher output");
    assert_eq!(first.snapshot.last_event_id.as_deref(), Some("evt-1"));
    assert_eq!(first.commands.len(), 1);

    supervisor
        .send(RuntimeEvent::Tick {
            now: "2026-03-11T00:00:21Z".to_owned(),
        })
        .await
        .expect("tick sends");

    let second = outputs.recv().await.expect("tick output");
    assert_eq!(second.snapshot.last_event_id.as_deref(), Some("evt-1"));
    assert_eq!(second.snapshot.watcher_states.len(), 1);
}

#[tokio::test]
async fn strategy_runtime_supervisor_uses_skip_for_missed_ticks() {
    let interval = supervisor_interval(Duration::from_millis(50));
    assert_eq!(interval.missed_tick_behavior(), MissedTickBehavior::Skip);
}

#[tokio::test]
async fn strategy_runtime_watchers_emit_data_events_not_direct_hedges() {
    let (supervisor, mut outputs) = runtime_supervisor().await;
    let sample = RuntimeWatcherState {
        watcher_key: "lp-position".to_owned(),
        metric: "delta_exposure_pct".to_owned(),
        value: 0.028,
        cursor: "evt-2".to_owned(),
        sampled_at: "2026-03-11T00:00:30Z".to_owned(),
    };

    supervisor
        .send(RuntimeEvent::WatcherSample(sample.clone()))
        .await
        .expect("watcher event sends");

    let output = outputs.recv().await.expect("runtime output");
    assert!(matches!(
        output.event,
        RuntimeEvent::WatcherSample(RuntimeWatcherState { .. })
    ));
    assert_eq!(output.snapshot.watcher_states, vec![sample]);
    assert_eq!(output.commands.len(), 1);
}

#[tokio::test]
async fn strategy_runtime_supervisor_is_consumed_by_daemon() {
    let (_data_dir, config, service) = setup_service().await;
    register_strategy(&service).await;

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let mut snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("snapshot exists");
    snapshot.runtime_state = "active".to_owned();
    snapshot.metrics = serde_json::json!({
        "warm": false,
        "venue_sync_required": false,
    });
    snapshot.updated_at = "2026-03-11T00:00:00Z".to_owned();
    repository
        .persist_strategy_recovery_snapshot(&snapshot)
        .await
        .expect("snapshot persists");

    let daemon = spawn_local_daemon_with_service(config.clone(), service)
        .await
        .expect("daemon boots with runtime supervisors");
    assert_eq!(
        daemon.active_runtime_supervisors(),
        vec!["strategy-lp-1".to_owned()]
    );

    daemon
        .publish_runtime_watcher_sample(
            "strategy-lp-1",
            RuntimeWatcherState {
                watcher_key: "lp-position".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-daemon-1".to_owned(),
                sampled_at: "2026-03-11T00:00:10Z".to_owned(),
            },
        )
        .await
        .expect("watcher sample routes through daemon supervisor");

    sleep(Duration::from_millis(150)).await;

    let snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot reloads")
        .expect("snapshot exists");
    let commands = daemon.take_runtime_commands("strategy-lp-1");

    assert_eq!(snapshot.last_event_id.as_deref(), Some("evt-daemon-1"));
    assert_eq!(snapshot.watcher_states.len(), 1);
    assert_eq!(snapshot.runtime_state, "rebalancing");
    assert!(!commands.is_empty());

    daemon.shutdown().await.expect("daemon shuts down");
}

async fn setup_service() -> (
    tempfile::TempDir,
    DaemonConfig,
    DaemonService<BaselinePolicy, SqliteReservationManager, PassiveSigner>,
) {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservations = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations open");
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(1_000),
        reservations,
        Arc::new(PassiveSigner),
    );
    (data_dir, config, service)
}

async fn register_strategy(
    service: &DaemonService<BaselinePolicy, SqliteReservationManager, PassiveSigner>,
) {
    let response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
}

fn strategy_request() -> JsonRpcRequest<serde_json::Value> {
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
                "watchers": [{
                    "watcher_type": "lp_position",
                    "source": "uniswap_v2",
                    "target": "TOKEN/USDT"
                }],
                "trigger_rules": [{
                    "trigger_type": "drift_threshold",
                    "metric": "delta_exposure_pct",
                    "operator": ">",
                    "value": "0.02",
                    "cooldown_sec": 10
                }],
                "calculation_model": {
                    "model_type": "delta_neutral_lp",
                    "inputs": ["lp_token_balance"]
                },
                "action_templates": [{
                    "action_type": "adjust_hedge",
                    "venue": "hyperliquid",
                    "instrument": "TOKEN-PERP",
                    "target": "delta_neutral"
                }],
                "constraints": {
                    "min_order_usd": 100,
                    "max_slippage_bps": 40,
                    "max_rebalances_per_hour": 60
                },
                "unwind_rules": [{"condition": "manual_stop"}]
            },
            "rationale": {"summary": "Keep delta neutral.", "main_risks": ["lag"]},
            "execution_preferences": {"preview_only": false, "allow_fast_path": false}
        }),
    )
}

async fn runtime_supervisor() -> (
    mpsc::Sender<RuntimeEvent>,
    mpsc::Receiver<a2ex_strategy_runtime::SupervisorOutput>,
) {
    let (event_tx, event_rx) = mpsc::channel(8);
    let (output_tx, output_rx) = mpsc::channel(8);
    tokio::spawn(async move {
        StrategySupervisor::new(runtime_snapshot())
            .run(event_rx, output_tx)
            .await
            .expect("supervisor runs");
    });

    (event_tx, output_rx)
}

fn runtime_snapshot() -> StrategyRuntimeSnapshot {
    StrategyRuntimeSnapshot {
        strategy: compile_strategy(&strategy_envelope()).expect("strategy compiles"),
        runtime_state: "active".to_owned(),
        next_tick_at: None,
        last_event_id: None,
        metrics: serde_json::json!({}),
        watcher_states: vec![],
        trigger_memory: vec![],
        pending_hedge: None,
    }
}

fn strategy_envelope() -> AgentRequestEnvelope<Strategy> {
    AgentRequestEnvelope {
        request_id: "req-loop-runtime".to_owned(),
        request_kind: AgentRequestKind::Strategy,
        source_agent_id: "agent-main".to_owned(),
        submitted_at: "2026-03-11T00:00:00Z".to_owned(),
        payload: Strategy {
            strategy_id: "strategy-lp-1".to_owned(),
            strategy_type: "stateful_hedge".to_owned(),
            watchers: vec![WatcherSpec {
                watcher_type: "lp_position".to_owned(),
                source: "local".to_owned(),
                chain: None,
                target: Some("TOKEN/USDT".to_owned()),
            }],
            trigger_rules: vec![TriggerRule {
                trigger_type: "drift_threshold".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                operator: ">".to_owned(),
                value: "0.02".to_owned(),
                cooldown_sec: Some(10),
            }],
            calculation_model: CalculationModel {
                model_type: "delta_neutral_lp".to_owned(),
                inputs: vec!["lp_token_balance".to_owned()],
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
            summary: "Keep delta neutral.".to_owned(),
            main_risks: vec![],
        },
        execution_preferences: ExecutionPreferences {
            preview_only: false,
            allow_fast_path: false,
            client_request_label: None,
        },
    }
}
