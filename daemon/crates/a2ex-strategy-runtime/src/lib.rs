use std::{collections::BTreeMap, time::Duration};

use a2ex_compiler::{CompiledStrategy, CompiledTriggerRule};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::mpsc,
    time::{Interval, MissedTickBehavior},
};

pub const MANUAL_STOP_METRIC: &str = "manual_stop";
pub const MANUAL_STOP_RUNTIME_STATE: &str = "unwinding";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeWatcherState {
    pub watcher_key: String,
    pub metric: String,
    pub value: f64,
    pub cursor: String,
    pub sampled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTriggerMemory {
    pub trigger_key: String,
    pub cooldown_until: Option<String>,
    pub last_fired_at: Option<String>,
    pub hysteresis_armed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePendingHedge {
    pub venue: String,
    pub instrument: String,
    pub client_order_id: String,
    pub signer_address: String,
    pub account_address: String,
    pub order_id: Option<u64>,
    pub nonce: u64,
    pub status: String,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyRuntimeSnapshot {
    pub strategy: CompiledStrategy,
    pub runtime_state: String,
    pub next_tick_at: Option<String>,
    pub last_event_id: Option<String>,
    pub metrics: serde_json::Value,
    pub watcher_states: Vec<RuntimeWatcherState>,
    pub trigger_memory: Vec<RuntimeTriggerMemory>,
    pub pending_hedge: Option<RuntimePendingHedge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuntimeEvent {
    WatcherSample(RuntimeWatcherState),
    Tick { now: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuntimeCommand {
    Rebalance(HedgeCommand),
    Unwind(HedgeCommand),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HedgeCommand {
    pub strategy_id: String,
    pub venue: String,
    pub instrument: String,
    pub notional_usd: u64,
    pub reduce_only: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub snapshot: StrategyRuntimeSnapshot,
    pub commands: Vec<RuntimeCommand>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupervisorOutput {
    pub event: RuntimeEvent,
    pub snapshot: StrategyRuntimeSnapshot,
    pub commands: Vec<RuntimeCommand>,
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("supervisor output channel closed")]
    OutputClosed,
}

pub struct StrategySupervisor {
    engine: StrategyRuntimeEngine,
    snapshot: StrategyRuntimeSnapshot,
    latest_samples: BTreeMap<String, RuntimeWatcherState>,
}

pub fn supervisor_interval(period: Duration) -> Interval {
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval
}

impl StrategySupervisor {
    pub fn new(snapshot: StrategyRuntimeSnapshot) -> Self {
        let latest_samples = snapshot
            .watcher_states
            .iter()
            .cloned()
            .map(|sample| (sample.watcher_key.clone(), sample))
            .collect();
        Self {
            engine: StrategyRuntimeEngine,
            snapshot,
            latest_samples,
        }
    }

    pub async fn run(
        mut self,
        mut event_rx: mpsc::Receiver<RuntimeEvent>,
        output_tx: mpsc::Sender<SupervisorOutput>,
    ) -> Result<(), SupervisorError> {
        while let Some(event) = event_rx.recv().await {
            let now = match &event {
                RuntimeEvent::WatcherSample(sample) => {
                    self.latest_samples
                        .insert(sample.watcher_key.clone(), sample.clone());
                    sample.sampled_at.clone()
                }
                RuntimeEvent::Tick { now } => now.clone(),
            };
            let samples = self.latest_samples.values().cloned().collect();
            let evaluation = self.engine.evaluate(self.snapshot.clone(), samples, &now);
            self.snapshot = evaluation.snapshot.clone();
            output_tx
                .send(SupervisorOutput {
                    event,
                    snapshot: evaluation.snapshot,
                    commands: evaluation.commands,
                })
                .await
                .map_err(|_| SupervisorError::OutputClosed)?;
        }

        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StrategyRuntimeEngine;

impl StrategyRuntimeEngine {
    pub fn restore(
        &self,
        mut snapshot: StrategyRuntimeSnapshot,
        recovered_at: &str,
    ) -> StrategyRuntimeSnapshot {
        snapshot.runtime_state = match snapshot.runtime_state.as_str() {
            "active" | "rebalancing" | "syncing_hedge" | "recovering" => "recovering".to_owned(),
            other => other.to_owned(),
        };
        snapshot.next_tick_at = Some(recovered_at.to_owned());
        set_metric_bool(&mut snapshot.metrics, "warm", true);
        set_metric_bool(&mut snapshot.metrics, "venue_sync_required", true);
        snapshot
    }

    pub fn evaluate(
        &self,
        mut snapshot: StrategyRuntimeSnapshot,
        samples: Vec<RuntimeWatcherState>,
        now: &str,
    ) -> EvaluationResult {
        snapshot.watcher_states = samples.clone();
        snapshot.last_event_id = samples.last().map(|sample| sample.cursor.clone());
        snapshot.next_tick_at = Some(now.to_owned());

        if metric_flag(&snapshot.metrics, "warm") {
            set_metric_bool(&mut snapshot.metrics, "warm", false);
            if metric_flag(&snapshot.metrics, "venue_sync_required") {
                snapshot.runtime_state = "syncing_hedge".to_owned();
            } else {
                snapshot.runtime_state = "active".to_owned();
            }
            return EvaluationResult {
                snapshot,
                commands: Vec::new(),
            };
        }

        if metric_flag(&snapshot.metrics, "venue_sync_required") {
            snapshot.runtime_state = "syncing_hedge".to_owned();
            return EvaluationResult {
                snapshot,
                commands: Vec::new(),
            };
        }

        let mut commands = Vec::new();
        if samples
            .iter()
            .any(|sample| sample.metric == MANUAL_STOP_METRIC && sample.value > 0.0)
        {
            commands.push(RuntimeCommand::Unwind(self.hedge_command(
                &snapshot,
                true,
                MANUAL_STOP_METRIC,
            )));
            snapshot.runtime_state = MANUAL_STOP_RUNTIME_STATE.to_owned();
            return EvaluationResult { snapshot, commands };
        }

        for (index, trigger) in snapshot
            .strategy
            .trigger_rules
            .clone()
            .into_iter()
            .enumerate()
        {
            let sample = match samples
                .iter()
                .find(|sample| sample.metric == trigger.metric)
            {
                Some(sample) => sample,
                None => continue,
            };
            let mut memory = trigger_memory(&snapshot, index);

            if !memory.hysteresis_armed && self.should_rearm(&trigger, sample.value) {
                memory.hysteresis_armed = true;
                upsert_trigger_memory(&mut snapshot, memory);
                continue;
            }

            if self.cooldown_active(&memory, now) || !memory.hysteresis_armed {
                continue;
            }

            if self.trigger_matches(&trigger, sample.value)
                && !self.rate_limit_active(&snapshot, now)
            {
                commands.push(RuntimeCommand::Rebalance(self.hedge_command(
                    &snapshot,
                    false,
                    &format!(
                        "{} {} {}",
                        trigger.metric, trigger.operator, trigger.threshold
                    ),
                )));
                snapshot.runtime_state = "rebalancing".to_owned();
                self.arm_trigger(&mut snapshot, &trigger, index, now);
                record_rebalance(&mut snapshot.metrics, now);
                break;
            }
        }

        EvaluationResult { snapshot, commands }
    }

    fn hedge_command(
        &self,
        snapshot: &StrategyRuntimeSnapshot,
        reduce_only: bool,
        reason: &str,
    ) -> HedgeCommand {
        let action = snapshot.strategy.action_templates[0].clone();
        let drift = snapshot
            .watcher_states
            .iter()
            .find(|sample| sample.metric == "delta_exposure_pct")
            .map(|sample| sample.value.abs())
            .unwrap_or(0.0);
        let notional = ((drift * 10_000.0) as u64).max(snapshot.strategy.constraints.min_order_usd);
        HedgeCommand {
            strategy_id: snapshot.strategy.strategy_id.clone(),
            venue: action.venue,
            instrument: action.instrument.unwrap_or_else(|| "unknown".to_owned()),
            notional_usd: notional,
            reduce_only,
            reason: reason.to_owned(),
        }
    }

    fn cooldown_active(&self, memory: &RuntimeTriggerMemory, now: &str) -> bool {
        memory
            .cooldown_until
            .as_ref()
            .is_some_and(|cooldown_until| now < cooldown_until.as_str())
    }

    fn arm_trigger(
        &self,
        snapshot: &mut StrategyRuntimeSnapshot,
        trigger: &CompiledTriggerRule,
        trigger_index: usize,
        now: &str,
    ) {
        let trigger_key = trigger_key(trigger_index);
        let cooldown_until = add_seconds(now, trigger.cooldown_sec as i64);
        if let Some(memory) = snapshot
            .trigger_memory
            .iter_mut()
            .find(|memory| memory.trigger_key == trigger_key)
        {
            memory.last_fired_at = Some(now.to_owned());
            memory.cooldown_until = cooldown_until;
            memory.hysteresis_armed = false;
            return;
        }

        snapshot.trigger_memory.push(RuntimeTriggerMemory {
            trigger_key,
            cooldown_until,
            last_fired_at: Some(now.to_owned()),
            hysteresis_armed: false,
        });
    }

    fn should_rearm(&self, trigger: &CompiledTriggerRule, value: f64) -> bool {
        let rearm_threshold = trigger.threshold * 0.75;
        match trigger.operator.as_str() {
            ">" | ">=" => value <= rearm_threshold,
            "<" | "<=" => value >= rearm_threshold,
            _ => false,
        }
    }

    fn rate_limit_active(&self, snapshot: &StrategyRuntimeSnapshot, now: &str) -> bool {
        let limit = snapshot.strategy.constraints.max_rebalances_per_hour;
        if limit == 0 {
            return false;
        }
        rebalance_history(&snapshot.metrics)
            .into_iter()
            .filter(|fired_at| within_last_hour(fired_at, now))
            .count()
            >= limit as usize
    }

    fn trigger_matches(&self, trigger: &CompiledTriggerRule, value: f64) -> bool {
        match trigger.operator.as_str() {
            ">" => value > trigger.threshold,
            ">=" => value >= trigger.threshold,
            "<" => value < trigger.threshold,
            "<=" => value <= trigger.threshold,
            _ => false,
        }
    }
}

fn trigger_key(index: usize) -> String {
    format!("trigger-{index}")
}

fn trigger_memory(
    snapshot: &StrategyRuntimeSnapshot,
    trigger_index: usize,
) -> RuntimeTriggerMemory {
    snapshot
        .trigger_memory
        .iter()
        .find(|memory| memory.trigger_key == trigger_key(trigger_index))
        .cloned()
        .unwrap_or(RuntimeTriggerMemory {
            trigger_key: trigger_key(trigger_index),
            cooldown_until: None,
            last_fired_at: None,
            hysteresis_armed: true,
        })
}

fn upsert_trigger_memory(snapshot: &mut StrategyRuntimeSnapshot, memory: RuntimeTriggerMemory) {
    if let Some(existing) = snapshot
        .trigger_memory
        .iter_mut()
        .find(|existing| existing.trigger_key == memory.trigger_key)
    {
        *existing = memory;
        return;
    }

    snapshot.trigger_memory.push(memory);
}

fn set_metric_bool(metrics: &mut serde_json::Value, key: &str, value: bool) {
    if let Some(object) = metrics.as_object_mut() {
        object.insert(key.to_owned(), serde_json::Value::Bool(value));
    } else {
        *metrics = serde_json::json!({ key: value });
    }
}

fn metric_flag(metrics: &serde_json::Value, key: &str) -> bool {
    metrics
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn record_rebalance(metrics: &mut serde_json::Value, now: &str) {
    let mut history = rebalance_history(metrics);
    history.push(now.to_owned());
    let values = history
        .into_iter()
        .filter(|fired_at| within_last_hour(fired_at, now))
        .map(serde_json::Value::String)
        .collect();
    if let Some(object) = metrics.as_object_mut() {
        object.insert(
            "rebalance_history".to_owned(),
            serde_json::Value::Array(values),
        );
    } else {
        *metrics = serde_json::json!({ "rebalance_history": values });
    }
}

fn rebalance_history(metrics: &serde_json::Value) -> Vec<String> {
    metrics
        .get("rebalance_history")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn within_last_hour(candidate: &str, now: &str) -> bool {
    let Some(candidate_epoch) = parse_rfc3339_utc(candidate) else {
        return false;
    };
    let Some(now_epoch) = parse_rfc3339_utc(now) else {
        return false;
    };
    (0..=3600).contains(&(now_epoch - candidate_epoch))
}

fn add_seconds(timestamp: &str, seconds: i64) -> Option<String> {
    parse_rfc3339_utc(timestamp).map(|epoch| format_rfc3339_utc(epoch + seconds))
}

fn parse_rfc3339_utc(timestamp: &str) -> Option<i64> {
    let timestamp = timestamp.strip_suffix('Z')?;
    let (date, time) = timestamp.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn format_rfc3339_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let seconds = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}
