use a2ex_compiler::compile_strategy;
use a2ex_control::{
    ActionTemplate, AgentRequestEnvelope, AgentRequestKind, CalculationModel, ExecutionPreferences,
    RationaleSummary, Strategy, StrategyConstraints, TriggerRule, UnwindRule, WatcherSpec,
};
use a2ex_strategy_runtime::{
    RuntimeTriggerMemory, RuntimeWatcherState, StrategyRuntimeEngine, StrategyRuntimeSnapshot,
};

#[test]
fn strategy_runtime_evaluates_threshold_cooldown_hysteresis() {
    let strategy = compile_strategy(&strategy_envelope()).expect("strategy compiles");
    let engine = StrategyRuntimeEngine;

    let firing = engine.evaluate(
        StrategyRuntimeSnapshot {
            strategy: strategy.clone(),
            runtime_state: "active".to_owned(),
            next_tick_at: None,
            last_event_id: None,
            metrics: serde_json::json!({}),
            watcher_states: vec![],
            trigger_memory: vec![],
            pending_hedge: None,
        },
        vec![RuntimeWatcherState {
            watcher_key: "w-1".to_owned(),
            metric: "delta_exposure_pct".to_owned(),
            value: 0.03,
            cursor: "evt-fire".to_owned(),
            sampled_at: "2026-03-11T00:02:00Z".to_owned(),
        }],
        "2026-03-11T00:02:00Z",
    );
    assert_eq!(firing.commands.len(), 1);
    assert_eq!(firing.snapshot.runtime_state, "rebalancing");
    assert_eq!(
        firing.snapshot.trigger_memory[0].cooldown_until.as_deref(),
        Some("2026-03-11T00:02:10Z")
    );

    let cooling = engine.evaluate(
        firing.snapshot.clone(),
        vec![RuntimeWatcherState {
            watcher_key: "w-1".to_owned(),
            metric: "delta_exposure_pct".to_owned(),
            value: 0.04,
            cursor: "evt-cooldown".to_owned(),
            sampled_at: "2026-03-11T00:02:05Z".to_owned(),
        }],
        "2026-03-11T00:02:05Z",
    );
    assert!(cooling.commands.is_empty());

    let hysteresis_hold = engine.evaluate(
        StrategyRuntimeSnapshot {
            strategy: strategy.clone(),
            runtime_state: "active".to_owned(),
            next_tick_at: None,
            last_event_id: None,
            metrics: serde_json::json!({}),
            watcher_states: vec![],
            trigger_memory: vec![RuntimeTriggerMemory {
                trigger_key: "trigger-0".to_owned(),
                cooldown_until: Some("2026-03-11T00:02:10Z".to_owned()),
                last_fired_at: Some("2026-03-11T00:02:00Z".to_owned()),
                hysteresis_armed: false,
            }],
            pending_hedge: None,
        },
        vec![RuntimeWatcherState {
            watcher_key: "w-1".to_owned(),
            metric: "delta_exposure_pct".to_owned(),
            value: 0.021,
            cursor: "evt-hold".to_owned(),
            sampled_at: "2026-03-11T00:02:11Z".to_owned(),
        }],
        "2026-03-11T00:02:11Z",
    );
    assert!(hysteresis_hold.commands.is_empty());
    assert!(!hysteresis_hold.snapshot.trigger_memory[0].hysteresis_armed);

    let rearm = engine.evaluate(
        hysteresis_hold.snapshot,
        vec![RuntimeWatcherState {
            watcher_key: "w-1".to_owned(),
            metric: "delta_exposure_pct".to_owned(),
            value: 0.015,
            cursor: "evt-rearm".to_owned(),
            sampled_at: "2026-03-11T00:02:20Z".to_owned(),
        }],
        "2026-03-11T00:02:20Z",
    );
    assert!(rearm.commands.is_empty());
    assert!(rearm.snapshot.trigger_memory[0].hysteresis_armed);

    let refire = engine.evaluate(
        rearm.snapshot,
        vec![RuntimeWatcherState {
            watcher_key: "w-1".to_owned(),
            metric: "delta_exposure_pct".to_owned(),
            value: 0.025,
            cursor: "evt-refire".to_owned(),
            sampled_at: "2026-03-11T00:02:21Z".to_owned(),
        }],
        "2026-03-11T00:02:21Z",
    );
    assert_eq!(refire.commands.len(), 1);

    let rate_limited = engine.evaluate(
        StrategyRuntimeSnapshot {
            strategy,
            runtime_state: "active".to_owned(),
            next_tick_at: None,
            last_event_id: None,
            metrics: serde_json::json!({
                "rebalance_history": [
                    "2026-03-10T23:10:00Z",
                    "2026-03-10T23:11:00Z",
                    "2026-03-10T23:12:00Z",
                    "2026-03-10T23:13:00Z",
                    "2026-03-10T23:14:00Z",
                    "2026-03-10T23:15:00Z",
                    "2026-03-10T23:16:00Z",
                    "2026-03-10T23:17:00Z",
                    "2026-03-10T23:18:00Z",
                    "2026-03-10T23:19:00Z",
                    "2026-03-10T23:20:00Z",
                    "2026-03-10T23:21:00Z",
                    "2026-03-10T23:22:00Z",
                    "2026-03-10T23:23:00Z",
                    "2026-03-10T23:24:00Z",
                    "2026-03-10T23:25:00Z",
                    "2026-03-10T23:26:00Z",
                    "2026-03-10T23:27:00Z",
                    "2026-03-10T23:28:00Z",
                    "2026-03-10T23:29:00Z",
                    "2026-03-10T23:30:00Z",
                    "2026-03-10T23:31:00Z",
                    "2026-03-10T23:32:00Z",
                    "2026-03-10T23:33:00Z",
                    "2026-03-10T23:34:00Z",
                    "2026-03-10T23:35:00Z",
                    "2026-03-10T23:36:00Z",
                    "2026-03-10T23:37:00Z",
                    "2026-03-10T23:38:00Z",
                    "2026-03-10T23:39:00Z",
                    "2026-03-10T23:40:00Z",
                    "2026-03-10T23:41:00Z",
                    "2026-03-10T23:42:00Z",
                    "2026-03-10T23:43:00Z",
                    "2026-03-10T23:44:00Z",
                    "2026-03-10T23:45:00Z",
                    "2026-03-10T23:46:00Z",
                    "2026-03-10T23:47:00Z",
                    "2026-03-10T23:48:00Z",
                    "2026-03-10T23:49:00Z",
                    "2026-03-10T23:50:00Z",
                    "2026-03-10T23:51:00Z",
                    "2026-03-10T23:52:00Z",
                    "2026-03-10T23:53:00Z",
                    "2026-03-10T23:54:00Z",
                    "2026-03-10T23:55:00Z",
                    "2026-03-10T23:56:00Z",
                    "2026-03-10T23:57:00Z",
                    "2026-03-10T23:58:00Z",
                    "2026-03-10T23:59:00Z",
                    "2026-03-11T00:00:00Z",
                    "2026-03-11T00:01:00Z",
                    "2026-03-11T00:02:00Z",
                    "2026-03-11T00:03:00Z",
                    "2026-03-11T00:04:00Z",
                    "2026-03-11T00:05:00Z",
                    "2026-03-11T00:06:00Z",
                    "2026-03-11T00:07:00Z",
                    "2026-03-11T00:08:00Z",
                    "2026-03-11T00:09:00Z"
                ]
            }),
            watcher_states: vec![],
            trigger_memory: vec![RuntimeTriggerMemory {
                trigger_key: "trigger-0".to_owned(),
                cooldown_until: None,
                last_fired_at: Some("2026-03-11T00:09:00Z".to_owned()),
                hysteresis_armed: true,
            }],
            pending_hedge: None,
        },
        vec![RuntimeWatcherState {
            watcher_key: "w-1".to_owned(),
            metric: "delta_exposure_pct".to_owned(),
            value: 0.04,
            cursor: "evt-rate-limit".to_owned(),
            sampled_at: "2026-03-11T00:09:30Z".to_owned(),
        }],
        "2026-03-11T00:09:30Z",
    );
    assert!(rate_limited.commands.is_empty());
}

fn strategy_envelope() -> AgentRequestEnvelope<Strategy> {
    AgentRequestEnvelope {
        request_id: "req-policy".to_owned(),
        request_kind: AgentRequestKind::Strategy,
        source_agent_id: "agent-main".to_owned(),
        submitted_at: "2026-03-11T00:00:00Z".to_owned(),
        payload: Strategy {
            strategy_id: "strategy-1".to_owned(),
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
