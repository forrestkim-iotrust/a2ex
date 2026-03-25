use a2ex_compiler::{CompiledAgentRequest, CompiledIntent, CompiledStrategy};
use a2ex_control::{RouteDecision, RouteTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoldReason {
    PreviewOnly,
}

impl HoldReason {
    fn summary(&self) -> &'static str {
        match self {
            Self::PreviewOnly => "preview-only request stops before execution handoff",
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::PreviewOnly => "preview_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastPathRoute {
    pub request_id: String,
    pub intent_id: String,
    pub venue: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedExecutionRoute {
    pub request_id: String,
    pub source_id: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatefulRuntimeRoute {
    pub request_id: String,
    pub strategy_id: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldRoute {
    pub request_id: String,
    pub source_id: String,
    pub reason: HoldReason,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "route", rename_all = "snake_case")]
pub enum GatewayVerdict {
    FastPath(FastPathRoute),
    PlannedExecution(PlannedExecutionRoute),
    StatefulRuntime(StatefulRuntimeRoute),
    Hold(HoldRoute),
}

impl GatewayVerdict {
    pub fn route_decision(&self) -> RouteDecision {
        match self {
            Self::FastPath(route) => RouteDecision {
                route: RouteTarget::FastPath,
                summary: route.summary.clone(),
                hold_reason: None,
            },
            Self::PlannedExecution(route) => RouteDecision {
                route: RouteTarget::PlannedExecution,
                summary: route.summary.clone(),
                hold_reason: None,
            },
            Self::StatefulRuntime(route) => RouteDecision {
                route: RouteTarget::StatefulRuntime,
                summary: route.summary.clone(),
                hold_reason: None,
            },
            Self::Hold(route) => RouteDecision {
                route: RouteTarget::Hold,
                summary: route.summary.clone(),
                hold_reason: Some(route.reason.code().to_owned()),
            },
        }
    }
}

pub fn classify(request: &CompiledAgentRequest) -> GatewayVerdict {
    match request {
        CompiledAgentRequest::Intent(intent) => classify_intent(intent),
        CompiledAgentRequest::Strategy(strategy) => classify_strategy(strategy),
    }
}

fn classify_intent(intent: &CompiledIntent) -> GatewayVerdict {
    if intent.audit.preview_only {
        return GatewayVerdict::Hold(HoldRoute {
            request_id: intent.audit.request_id.clone(),
            source_id: intent.intent_id.clone(),
            reason: HoldReason::PreviewOnly,
            summary: HoldReason::PreviewOnly.summary().to_owned(),
        });
    }

    if is_fast_path_eligible(intent) {
        return GatewayVerdict::FastPath(FastPathRoute {
            request_id: intent.audit.request_id.clone(),
            intent_id: intent.intent_id.clone(),
            venue: intent.constraints.allowed_venues[0].clone(),
            summary: format!(
                "deterministic single-venue intent {} can use fast path",
                intent.intent_id
            ),
        });
    }

    GatewayVerdict::PlannedExecution(PlannedExecutionRoute {
        request_id: intent.audit.request_id.clone(),
        source_id: intent.intent_id.clone(),
        summary: format!(
            "intent {} needs planned execution because it spans multiple routing concerns",
            intent.intent_id
        ),
    })
}

fn classify_strategy(strategy: &CompiledStrategy) -> GatewayVerdict {
    if strategy.audit.preview_only {
        return GatewayVerdict::Hold(HoldRoute {
            request_id: strategy.audit.request_id.clone(),
            source_id: strategy.strategy_id.clone(),
            reason: HoldReason::PreviewOnly,
            summary: HoldReason::PreviewOnly.summary().to_owned(),
        });
    }

    GatewayVerdict::StatefulRuntime(StatefulRuntimeRoute {
        request_id: strategy.audit.request_id.clone(),
        strategy_id: strategy.strategy_id.clone(),
        summary: format!(
            "strategy {} requires stateful runtime orchestration",
            strategy.strategy_id
        ),
    })
}

fn is_fast_path_eligible(intent: &CompiledIntent) -> bool {
    intent.constraints.allowed_venues.len() == 1
        && intent.constraints.hedge_ratio_bps == 0
        && intent.post_actions.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2ex_compiler::{
        CompileAuditContext, CompiledFunding, CompiledIntentConstraints, CompiledIntentObjective,
        CompiledPostAction, CompiledStrategyConstraints,
    };
    use a2ex_control::ExecutionUrgency;

    #[test]
    fn classifies_single_venue_intent_as_fast_path() {
        let verdict = classify(&CompiledAgentRequest::Intent(CompiledIntent {
            audit: audit("req-fast", false),
            intent_id: "intent-fast".to_owned(),
            intent_type: "open_exposure".to_owned(),
            objective: CompiledIntentObjective {
                domain: "prediction_market".to_owned(),
                target_market: "market".to_owned(),
                side: "yes".to_owned(),
                target_notional_usd: 1_000,
            },
            constraints: CompiledIntentConstraints {
                allowed_venues: vec!["polymarket".to_owned()],
                max_slippage_bps: 80,
                max_fee_usd: 25,
                urgency: ExecutionUrgency::High,
                hedge_ratio_bps: 0,
            },
            funding: CompiledFunding {
                preferred_asset: "USDC".to_owned(),
                source_chain: "base".to_owned(),
            },
            post_actions: vec![],
        }));

        assert!(matches!(verdict, GatewayVerdict::FastPath(_)));
        assert_eq!(verdict.route_decision().route, RouteTarget::FastPath);
    }

    #[test]
    fn classifies_complex_intent_as_planned_execution() {
        let verdict = classify(&CompiledAgentRequest::Intent(CompiledIntent {
            audit: audit("req-plan", false),
            intent_id: "intent-plan".to_owned(),
            intent_type: "open_exposure".to_owned(),
            objective: CompiledIntentObjective {
                domain: "prediction_market".to_owned(),
                target_market: "market".to_owned(),
                side: "yes".to_owned(),
                target_notional_usd: 1_000,
            },
            constraints: CompiledIntentConstraints {
                allowed_venues: vec!["kalshi".to_owned(), "polymarket".to_owned()],
                max_slippage_bps: 80,
                max_fee_usd: 25,
                urgency: ExecutionUrgency::Normal,
                hedge_ratio_bps: 4_000,
            },
            funding: CompiledFunding {
                preferred_asset: "USDC".to_owned(),
                source_chain: "base".to_owned(),
            },
            post_actions: vec![CompiledPostAction {
                action_type: "hedge".to_owned(),
                venue: "hyperliquid".to_owned(),
            }],
        }));

        assert!(matches!(verdict, GatewayVerdict::PlannedExecution(_)));
        assert_eq!(
            verdict.route_decision().route,
            RouteTarget::PlannedExecution
        );
    }

    #[test]
    fn classifies_strategies_and_preview_requests_without_side_effects() {
        let strategy_verdict = classify(&CompiledAgentRequest::Strategy(CompiledStrategy {
            audit: audit("req-strategy", false),
            strategy_id: "strategy-1".to_owned(),
            strategy_type: "stateful_hedge".to_owned(),
            watchers: vec![],
            trigger_rules: vec![],
            calculation_model: a2ex_compiler::CompiledCalculationModel {
                model_type: "delta_neutral_lp".to_owned(),
                inputs: vec![],
            },
            action_templates: vec![],
            constraints: CompiledStrategyConstraints {
                min_order_usd: 1,
                max_slippage_bps: 40,
                max_rebalances_per_hour: 60,
            },
            unwind_conditions: vec!["manual_stop".to_owned()],
        }));
        assert!(matches!(
            strategy_verdict,
            GatewayVerdict::StatefulRuntime(_)
        ));

        let hold_verdict = classify(&CompiledAgentRequest::Intent(CompiledIntent {
            audit: audit("req-preview", true),
            intent_id: "intent-preview".to_owned(),
            intent_type: "open_exposure".to_owned(),
            objective: CompiledIntentObjective {
                domain: "prediction_market".to_owned(),
                target_market: "market".to_owned(),
                side: "yes".to_owned(),
                target_notional_usd: 1_000,
            },
            constraints: CompiledIntentConstraints {
                allowed_venues: vec!["polymarket".to_owned()],
                max_slippage_bps: 80,
                max_fee_usd: 25,
                urgency: ExecutionUrgency::Low,
                hedge_ratio_bps: 0,
            },
            funding: CompiledFunding {
                preferred_asset: "USDC".to_owned(),
                source_chain: "base".to_owned(),
            },
            post_actions: vec![],
        }));

        assert!(matches!(hold_verdict, GatewayVerdict::Hold(_)));
        assert_eq!(
            hold_verdict.route_decision().hold_reason.as_deref(),
            Some("preview_only")
        );
    }

    fn audit(request_id: &str, preview_only: bool) -> CompileAuditContext {
        CompileAuditContext {
            request_id: request_id.to_owned(),
            source_agent_id: "agent-main".to_owned(),
            submitted_at: "2026-03-11T00:00:00Z".to_owned(),
            rationale_summary: "keep routing explicit".to_owned(),
            preview_only,
            client_request_label: None,
        }
    }
}
