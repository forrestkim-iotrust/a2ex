use a2ex_control::{
    ActionTemplate, AgentRequestEnvelope, ExecutionUrgency, Intent, PostAction, Strategy,
    TriggerRule, WatcherSpec,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_INTENT_VENUES: [&str; 4] = ["polymarket", "kalshi", "hyperliquid", "across"];
const DEFAULT_MAX_REBALANCES_PER_HOUR: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileAuditContext {
    pub request_id: String,
    pub source_agent_id: String,
    pub submitted_at: String,
    pub rationale_summary: String,
    pub preview_only: bool,
    pub client_request_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledIntent {
    pub audit: CompileAuditContext,
    pub intent_id: String,
    pub intent_type: String,
    pub objective: CompiledIntentObjective,
    pub constraints: CompiledIntentConstraints,
    pub funding: CompiledFunding,
    pub post_actions: Vec<CompiledPostAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledIntentObjective {
    pub domain: String,
    pub target_market: String,
    pub side: String,
    pub target_notional_usd: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledIntentConstraints {
    pub allowed_venues: Vec<String>,
    pub max_slippage_bps: u64,
    pub max_fee_usd: u64,
    pub urgency: ExecutionUrgency,
    pub hedge_ratio_bps: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledFunding {
    pub preferred_asset: String,
    pub source_chain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledPostAction {
    pub action_type: String,
    pub venue: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledStrategy {
    pub audit: CompileAuditContext,
    pub strategy_id: String,
    pub strategy_type: String,
    pub watchers: Vec<CompiledWatcher>,
    pub trigger_rules: Vec<CompiledTriggerRule>,
    pub calculation_model: CompiledCalculationModel,
    pub action_templates: Vec<CompiledActionTemplate>,
    pub constraints: CompiledStrategyConstraints,
    pub unwind_conditions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledWatcher {
    pub watcher_type: String,
    pub source: String,
    pub chain: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledTriggerRule {
    pub trigger_type: String,
    pub metric: String,
    pub operator: String,
    pub threshold: f64,
    pub cooldown_sec: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledCalculationModel {
    pub model_type: String,
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledActionTemplate {
    pub action_type: String,
    pub venue: String,
    pub instrument: Option<String>,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledStrategyConstraints {
    pub min_order_usd: u64,
    pub max_slippage_bps: u64,
    pub max_rebalances_per_hour: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompiledAgentRequest {
    Intent(CompiledIntent),
    Strategy(CompiledStrategy),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerIssue {
    pub field: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerFailure {
    pub issues: Vec<CompilerIssue>,
}

impl CompilerFailure {
    fn single(
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            issues: vec![CompilerIssue {
                field: field.into(),
                code: code.into(),
                message: message.into(),
            }],
        }
    }

    pub fn issues(&self) -> &[CompilerIssue] {
        &self.issues
    }
}

#[derive(Debug, Error)]
#[error("compiler rejected request")]
pub struct CompilerError {
    failure: CompilerFailure,
}

impl CompilerError {
    fn single(
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            failure: CompilerFailure::single(field, code, message),
        }
    }

    pub fn failure(&self) -> &CompilerFailure {
        &self.failure
    }

    fn from_issues(issues: Vec<CompilerIssue>) -> Self {
        Self {
            failure: CompilerFailure { issues },
        }
    }
}

pub fn compile_intent(
    envelope: &AgentRequestEnvelope<Intent>,
) -> Result<CompiledIntent, CompilerError> {
    validate_envelope(
        envelope.request_id.as_str(),
        envelope.source_agent_id.as_str(),
        envelope.submitted_at.as_str(),
        envelope.rationale.summary.as_str(),
    )?;

    if envelope.payload.objective.target_notional_usd == 0 {
        return Err(CompilerError::single(
            "payload.objective.target_notional_usd",
            "invalid_notional",
            "target_notional_usd must be greater than zero",
        ));
    }
    if envelope.payload.constraints.max_slippage_bps == 0
        || envelope.payload.constraints.max_slippage_bps > 10_000
    {
        return Err(CompilerError::single(
            "payload.constraints.max_slippage_bps",
            "invalid_constraint",
            "max_slippage_bps must be between 1 and 10000",
        ));
    }

    let post_actions = envelope
        .payload
        .post_actions
        .iter()
        .map(compile_post_action)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CompiledIntent {
        audit: compile_audit(envelope),
        intent_id: normalize_token(&envelope.payload.intent_id),
        intent_type: normalize_token(&envelope.payload.intent_type),
        objective: CompiledIntentObjective {
            domain: normalize_token(&envelope.payload.objective.domain),
            target_market: normalize_token(&envelope.payload.objective.target_market),
            side: normalize_token(&envelope.payload.objective.side),
            target_notional_usd: envelope.payload.objective.target_notional_usd,
        },
        constraints: CompiledIntentConstraints {
            allowed_venues: normalize_venues(&envelope.payload.constraints.allowed_venues),
            max_slippage_bps: envelope.payload.constraints.max_slippage_bps,
            max_fee_usd: envelope.payload.constraints.max_fee_usd,
            urgency: envelope.payload.constraints.urgency.clone(),
            hedge_ratio_bps: envelope.payload.constraints.hedge_ratio_bps.unwrap_or(0),
        },
        funding: CompiledFunding {
            preferred_asset: envelope
                .payload
                .funding
                .preferred_asset
                .trim()
                .to_ascii_uppercase(),
            source_chain: normalize_token(&envelope.payload.funding.source_chain),
        },
        post_actions,
    })
}

pub fn compile_strategy(
    envelope: &AgentRequestEnvelope<Strategy>,
) -> Result<CompiledStrategy, CompilerError> {
    validate_envelope(
        envelope.request_id.as_str(),
        envelope.source_agent_id.as_str(),
        envelope.submitted_at.as_str(),
        envelope.rationale.summary.as_str(),
    )?;

    let mut issues = Vec::new();

    if envelope.payload.watchers.is_empty() {
        issues.push(CompilerIssue {
            field: "payload.watchers".to_owned(),
            code: "missing_required".to_owned(),
            message: "strategy must include at least one watcher".to_owned(),
        });
    }
    if envelope.payload.trigger_rules.is_empty() {
        issues.push(CompilerIssue {
            field: "payload.trigger_rules".to_owned(),
            code: "missing_required".to_owned(),
            message: "strategy must include at least one trigger rule".to_owned(),
        });
    }
    if envelope.payload.action_templates.is_empty() {
        issues.push(CompilerIssue {
            field: "payload.action_templates".to_owned(),
            code: "missing_required".to_owned(),
            message: "strategy must include at least one action template".to_owned(),
        });
    }
    if envelope.payload.constraints.max_slippage_bps == 0
        || envelope.payload.constraints.max_slippage_bps > 10_000
    {
        issues.push(CompilerIssue {
            field: "payload.constraints.max_slippage_bps".to_owned(),
            code: "invalid_constraint".to_owned(),
            message: "max_slippage_bps must be between 1 and 10000".to_owned(),
        });
    }
    if matches!(
        envelope.payload.constraints.max_rebalances_per_hour,
        Some(0)
    ) {
        issues.push(CompilerIssue {
            field: "payload.constraints.max_rebalances_per_hour".to_owned(),
            code: "invalid_constraint".to_owned(),
            message: "max_rebalances_per_hour must be greater than zero when provided".to_owned(),
        });
    }

    let watchers = envelope
        .payload
        .watchers
        .iter()
        .map(compile_watcher)
        .collect::<Result<Vec<_>, _>>()?;
    let trigger_rules = envelope
        .payload
        .trigger_rules
        .iter()
        .map(compile_trigger_rule)
        .collect::<Result<Vec<_>, _>>()?;
    let action_templates = envelope
        .payload
        .action_templates
        .iter()
        .map(|action| compile_action_template(action, &mut issues))
        .collect::<Result<Vec<_>, _>>()?;

    if !issues.is_empty() {
        return Err(CompilerError::from_issues(issues));
    }

    let unwind_conditions = if envelope.payload.unwind_rules.is_empty() {
        vec!["manual_stop".to_owned()]
    } else {
        envelope
            .payload
            .unwind_rules
            .iter()
            .map(|rule| normalize_token(&rule.condition))
            .collect()
    };

    Ok(CompiledStrategy {
        audit: compile_audit(envelope),
        strategy_id: normalize_token(&envelope.payload.strategy_id),
        strategy_type: normalize_token(&envelope.payload.strategy_type),
        watchers,
        trigger_rules,
        calculation_model: CompiledCalculationModel {
            model_type: normalize_token(&envelope.payload.calculation_model.model_type),
            inputs: envelope
                .payload
                .calculation_model
                .inputs
                .iter()
                .map(|input| normalize_token(input))
                .collect(),
        },
        action_templates,
        constraints: CompiledStrategyConstraints {
            min_order_usd: envelope.payload.constraints.min_order_usd.unwrap_or(1),
            max_slippage_bps: envelope.payload.constraints.max_slippage_bps,
            max_rebalances_per_hour: envelope
                .payload
                .constraints
                .max_rebalances_per_hour
                .unwrap_or(DEFAULT_MAX_REBALANCES_PER_HOUR),
        },
        unwind_conditions,
    })
}

pub fn format_compiler_failure(error: &CompilerError) -> String {
    serde_json::to_string(error.failure()).unwrap_or_else(|_| {
        "{\"issues\":[{\"code\":\"compiler_failure\",\"message\":\"request rejected\"}]}".to_owned()
    })
}

fn compile_audit<T>(envelope: &AgentRequestEnvelope<T>) -> CompileAuditContext {
    CompileAuditContext {
        request_id: envelope.request_id.trim().to_owned(),
        source_agent_id: envelope.source_agent_id.trim().to_owned(),
        submitted_at: envelope.submitted_at.trim().to_owned(),
        rationale_summary: envelope.rationale.summary.trim().to_owned(),
        preview_only: envelope.execution_preferences.preview_only,
        client_request_label: envelope
            .execution_preferences
            .client_request_label
            .as_ref()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()),
    }
}

fn validate_envelope(
    request_id: &str,
    source_agent_id: &str,
    submitted_at: &str,
    rationale_summary: &str,
) -> Result<(), CompilerError> {
    if request_id.trim().is_empty() {
        return Err(CompilerError::single(
            "request_id",
            "missing_required",
            "request_id must not be empty",
        ));
    }
    if source_agent_id.trim().is_empty() {
        return Err(CompilerError::single(
            "source_agent_id",
            "missing_required",
            "source_agent_id must not be empty",
        ));
    }
    if submitted_at.trim().is_empty() {
        return Err(CompilerError::single(
            "submitted_at",
            "missing_required",
            "submitted_at must not be empty",
        ));
    }
    if rationale_summary.trim().is_empty() {
        return Err(CompilerError::single(
            "rationale.summary",
            "missing_required",
            "rationale.summary must not be empty",
        ));
    }
    Ok(())
}

fn compile_post_action(action: &PostAction) -> Result<CompiledPostAction, CompilerError> {
    let action_type = normalize_token(&action.action_type);
    if !matches!(action_type.as_str(), "hedge" | "bridge") {
        return Err(CompilerError::single(
            "payload.post_actions.action_type",
            "unsupported_action_type",
            format!("unsupported post action type: {action_type}"),
        ));
    }

    Ok(CompiledPostAction {
        action_type,
        venue: normalize_token(&action.venue),
    })
}

fn compile_watcher(watcher: &WatcherSpec) -> Result<CompiledWatcher, CompilerError> {
    let watcher_type = normalize_token(&watcher.watcher_type);
    if watcher_type.is_empty() {
        return Err(CompilerError::single(
            "payload.watchers.watcher_type",
            "missing_required",
            "watcher_type must not be empty",
        ));
    }

    Ok(CompiledWatcher {
        watcher_type,
        source: normalize_token(&watcher.source),
        chain: watcher.chain.as_ref().map(|chain| normalize_token(chain)),
        target: watcher
            .target
            .as_ref()
            .map(|target| target.trim().to_owned()),
    })
}

fn compile_trigger_rule(rule: &TriggerRule) -> Result<CompiledTriggerRule, CompilerError> {
    let operator = rule.operator.trim();
    if !matches!(operator, ">" | ">=" | "<" | "<=") {
        return Err(CompilerError::single(
            "payload.trigger_rules.operator",
            "unsupported_operator",
            format!("unsupported trigger operator: {operator}"),
        ));
    }

    let threshold = rule.value.trim().parse::<f64>().map_err(|_| {
        CompilerError::single(
            "payload.trigger_rules.value",
            "invalid_threshold",
            format!("trigger threshold must be numeric: {}", rule.value),
        )
    })?;

    Ok(CompiledTriggerRule {
        trigger_type: normalize_token(&rule.trigger_type),
        metric: normalize_token(&rule.metric),
        operator: operator.to_owned(),
        threshold,
        cooldown_sec: rule.cooldown_sec.unwrap_or(0),
    })
}

fn compile_action_template(
    action: &ActionTemplate,
    issues: &mut Vec<CompilerIssue>,
) -> Result<CompiledActionTemplate, CompilerError> {
    let action_type = normalize_token(&action.action_type);
    if !matches!(action_type.as_str(), "adjust_hedge") {
        issues.push(CompilerIssue {
            field: "payload.action_templates.action_type".to_owned(),
            code: "unsupported_action_type".to_owned(),
            message: format!("unsupported action template type: {action_type}"),
        });
    }

    Ok(CompiledActionTemplate {
        action_type,
        venue: normalize_token(&action.venue),
        instrument: action
            .instrument
            .as_ref()
            .map(|value| value.trim().to_owned()),
        target: action.target.as_ref().map(|value| normalize_token(value)),
    })
}

fn normalize_venues(venues: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = if venues.is_empty() {
        DEFAULT_INTENT_VENUES
            .iter()
            .map(|venue| (*venue).to_owned())
            .collect()
    } else {
        venues.iter().map(|venue| normalize_token(venue)).collect()
    };
    normalized.sort();
    normalized.dedup();
    normalized
}

fn normalize_token(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2ex_control::{
        AgentRequestEnvelope, AgentRequestKind, CalculationModel, ExecutionPreferences,
        IntentConstraints, IntentFunding, IntentObjective, RationaleSummary, StrategyConstraints,
        UnwindRule,
    };

    #[test]
    fn compiler_fills_defaults_and_normalizes_request_shapes() {
        let compiled = compile_intent(&AgentRequestEnvelope {
            request_id: " req-1 ".to_owned(),
            request_kind: AgentRequestKind::Intent,
            source_agent_id: " agent-main ".to_owned(),
            submitted_at: "2026-03-11T00:00:00Z".to_owned(),
            payload: Intent {
                intent_id: " Intent-1 ".to_owned(),
                intent_type: " Open_Exposure ".to_owned(),
                objective: IntentObjective {
                    domain: " Prediction_Market ".to_owned(),
                    target_market: " US-Election-2028 ".to_owned(),
                    side: " YES ".to_owned(),
                    target_notional_usd: 3000,
                },
                constraints: IntentConstraints {
                    allowed_venues: vec![],
                    max_slippage_bps: 80,
                    max_fee_usd: 25,
                    urgency: ExecutionUrgency::Normal,
                    hedge_ratio_bps: None,
                },
                funding: IntentFunding {
                    preferred_asset: "usdc".to_owned(),
                    source_chain: " Base ".to_owned(),
                },
                post_actions: vec![],
            },
            rationale: RationaleSummary {
                summary: "Keep it auditable".to_owned(),
                main_risks: vec![],
            },
            execution_preferences: ExecutionPreferences {
                preview_only: false,
                allow_fast_path: true,
                client_request_label: None,
            },
        })
        .expect("intent compiles");

        assert_eq!(compiled.intent_id, "intent-1");
        assert_eq!(
            compiled.constraints.allowed_venues,
            vec!["across", "hyperliquid", "kalshi", "polymarket"]
        );
        assert_eq!(compiled.constraints.hedge_ratio_bps, 0);
        assert_eq!(compiled.funding.preferred_asset, "USDC");
        assert_eq!(compiled.audit.rationale_summary, "Keep it auditable");
    }

    #[test]
    fn compiler_rejects_unsupported_strategy_actions() {
        let error = compile_strategy(&AgentRequestEnvelope {
            request_id: "req-2".to_owned(),
            request_kind: AgentRequestKind::Strategy,
            source_agent_id: "agent-main".to_owned(),
            submitted_at: "2026-03-11T00:00:00Z".to_owned(),
            payload: Strategy {
                strategy_id: "strategy-1".to_owned(),
                strategy_type: "stateful_hedge".to_owned(),
                watchers: vec![WatcherSpec {
                    watcher_type: "lp_position".to_owned(),
                    source: "uniswap_v2".to_owned(),
                    chain: Some("arbitrum".to_owned()),
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
                    action_type: "call_contract".to_owned(),
                    venue: "hyperliquid".to_owned(),
                    instrument: None,
                    target: None,
                }],
                constraints: StrategyConstraints {
                    min_order_usd: None,
                    max_slippage_bps: 40,
                    max_rebalances_per_hour: None,
                },
                unwind_rules: vec![UnwindRule {
                    condition: "manual_stop".to_owned(),
                }],
            },
            rationale: RationaleSummary {
                summary: "Stay hedged".to_owned(),
                main_risks: vec![],
            },
            execution_preferences: ExecutionPreferences {
                preview_only: false,
                allow_fast_path: false,
                client_request_label: None,
            },
        })
        .expect_err("unsupported action should fail");

        assert_eq!(error.failure().issues()[0].code, "unsupported_action_type");
    }
}
