use a2ex_compiler::CompiledIntent;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityMatrix {
    pub venues: Vec<VenueCapability>,
}

impl CapabilityMatrix {
    pub fn m001_defaults() -> Self {
        Self {
            venues: vec![
                VenueCapability {
                    venue: "across".to_owned(),
                    kind: VenueKind::Bridge,
                    supported_step_types: vec!["bridge_asset".to_owned()],
                    supported_chains: vec!["base".to_owned(), "arbitrum".to_owned(), "polygon".to_owned()],
                    approval_requirements: vec![ApprovalRequirement {
                        approval_type: "erc20_allowance".to_owned(),
                        asset: Some("USDC".to_owned()),
                        context: Some("bridge_submit".to_owned()),
                        required: true,
                        summary: "Across bridge deposits require token allowance approval before submit." .to_owned(),
                    }],
                    auth_summary: "Onchain bridge deposit signed locally through signer bridge." .to_owned(),
                },
                VenueCapability {
                    venue: "polymarket".to_owned(),
                    kind: VenueKind::PredictionMarket,
                    supported_step_types: vec!["place_order".to_owned(), "query_order_state".to_owned()],
                    supported_chains: vec!["polygon".to_owned()],
                    approval_requirements: vec![ApprovalRequirement {
                        approval_type: "order_signature".to_owned(),
                        asset: Some("USDC".to_owned()),
                        context: Some("entry_order_submit".to_owned()),
                        required: true,
                        summary: "Polymarket orders require locally-derived auth plus signed order payloads." .to_owned(),
                    }],
                    auth_summary: "Local L1 wallet auth derives and signs CLOB order payloads." .to_owned(),
                },
                VenueCapability {
                    venue: "kalshi".to_owned(),
                    kind: VenueKind::PredictionMarket,
                    supported_step_types: vec!["place_order".to_owned(), "query_order_state".to_owned()],
                    supported_chains: vec![],
                    approval_requirements: vec![ApprovalRequirement {
                        approval_type: "api_request_signature".to_owned(),
                        asset: None,
                        context: Some("entry_order_submit".to_owned()),
                        required: true,
                        summary: "Kalshi REST requests require signed local API authentication." .to_owned(),
                    }],
                    auth_summary: "Local API key auth signs each order request without moving credentials remote." .to_owned(),
                },
                VenueCapability {
                    venue: "hyperliquid".to_owned(),
                    kind: VenueKind::Hedge,
                    supported_step_types: vec!["adjust_hedge".to_owned(), "query_position".to_owned()],
                    supported_chains: vec![],
                    approval_requirements: vec![ApprovalRequirement {
                        approval_type: "exchange_order_signature".to_owned(),
                        asset: None,
                        context: Some("hedge_order_submit".to_owned()),
                        required: true,
                        summary: "Hyperliquid hedge orders are signed locally with signer/account separation." .to_owned(),
                    }],
                    auth_summary: "Locally-signed exchange payloads with distinct signer and account identities." .to_owned(),
                },
            ],
        }
    }

    pub fn venue(&self, venue: &str) -> Option<&VenueCapability> {
        self.venues
            .iter()
            .find(|capability| capability.venue == venue)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueKind {
    Bridge,
    PredictionMarket,
    Hedge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequirement {
    pub approval_type: String,
    pub asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub required: bool,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueCapability {
    pub venue: String,
    pub kind: VenueKind,
    pub supported_step_types: Vec<String>,
    pub supported_chains: Vec<String>,
    pub approval_requirements: Vec<ApprovalRequirement>,
    pub auth_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedRoute {
    pub bridge_venue: Option<String>,
    pub entry_venue: String,
    pub hedge_venue: Option<String>,
    pub destination_chain: Option<String>,
    pub fallback_entry_venue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u8,
    pub backoff_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    Abort,
    Retry,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepFailurePolicy {
    pub mode: FailureMode,
    pub retry: RetryPolicy,
    pub fallback_venue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeStepParams {
    pub asset: String,
    pub amount_usd: u64,
    pub source_chain: String,
    pub destination_chain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryStepParams {
    pub market: String,
    pub side: String,
    pub notional_usd: u64,
    pub max_fee_usd: u64,
    pub max_slippage_bps: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HedgeStepParams {
    pub instrument: String,
    pub notional_usd: u64,
    pub reduce_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanStepParams {
    Bridge(BridgeStepParams),
    Entry(EntryStepParams),
    Hedge(HedgeStepParams),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_id: String,
    pub sequence: u32,
    pub step_type: String,
    pub adapter: String,
    pub approval_required: bool,
    pub idempotency_key: String,
    pub failure_policy: StepFailurePolicy,
    pub params: PlanStepParams,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRiskSummary {
    pub expected_fee_usd: u64,
    pub expected_slippage_bps: u64,
    pub residual_unhedged_exposure_usd: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub plan_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub request_id: String,
    pub summary: String,
    pub route: PlannedRoute,
    pub steps: Vec<PlanStep>,
    pub risk_summary: PlanRiskSummary,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PlannerError {
    #[error("intent {intent_id} does not contain a prediction-market venue we can route")]
    NoPredictionVenue { intent_id: String },
    #[error("intent {intent_id} requested unsupported venue {venue}")]
    UnsupportedVenue { intent_id: String, venue: String },
    #[error("intent {intent_id} requested hedge venue {venue} without capability metadata")]
    UnsupportedHedgeVenue { intent_id: String, venue: String },
}

pub fn plan_intent(
    intent: &CompiledIntent,
    matrix: &CapabilityMatrix,
) -> Result<ExecutionPlan, PlannerError> {
    let prediction_venues = prediction_candidates(intent, matrix)?;
    let primary_entry = prediction_venues[0].clone();
    let fallback_entry = prediction_venues.get(1).cloned();
    let destination_chain = matrix
        .venue(&primary_entry)
        .and_then(|capability| capability.supported_chains.first().cloned());
    let bridge_required = destination_chain
        .as_ref()
        .is_some_and(|chain| chain != &intent.funding.source_chain);
    let hedge_venue = hedge_venue(intent, matrix)?;
    let bridge_amount = intent.objective.target_notional_usd;
    let hedge_notional =
        intent.objective.target_notional_usd * intent.constraints.hedge_ratio_bps / 10_000;
    let plan_id = format!("plan-{}", intent.intent_id);
    let mut steps = Vec::new();

    if bridge_required {
        steps.push(PlanStep {
            step_id: format!("{plan_id}:bridge"),
            sequence: (steps.len() + 1) as u32,
            step_type: "bridge_asset".to_owned(),
            adapter: "across".to_owned(),
            approval_required: true,
            idempotency_key: format!("{}:bridge", intent.audit.request_id),
            failure_policy: StepFailurePolicy {
                mode: FailureMode::Retry,
                retry: RetryPolicy {
                    max_attempts: 2,
                    backoff_ms: 250,
                },
                fallback_venue: None,
            },
            params: PlanStepParams::Bridge(BridgeStepParams {
                asset: intent.funding.preferred_asset.clone(),
                amount_usd: bridge_amount,
                source_chain: intent.funding.source_chain.clone(),
                destination_chain: destination_chain
                    .clone()
                    .expect("bridge requires destination chain"),
            }),
        });
    }

    steps.push(PlanStep {
        step_id: format!("{plan_id}:entry"),
        sequence: (steps.len() + 1) as u32,
        step_type: "place_order".to_owned(),
        adapter: primary_entry.clone(),
        approval_required: true,
        idempotency_key: format!("{}:entry:{}", intent.audit.request_id, primary_entry),
        failure_policy: StepFailurePolicy {
            mode: if fallback_entry.is_some() {
                FailureMode::Fallback
            } else {
                FailureMode::Retry
            },
            retry: RetryPolicy {
                max_attempts: if fallback_entry.is_some() { 1 } else { 2 },
                backoff_ms: 200,
            },
            fallback_venue: fallback_entry.clone(),
        },
        params: PlanStepParams::Entry(EntryStepParams {
            market: intent.objective.target_market.clone(),
            side: intent.objective.side.clone(),
            notional_usd: intent
                .objective
                .target_notional_usd
                .saturating_sub(hedge_notional),
            max_fee_usd: intent.constraints.max_fee_usd,
            max_slippage_bps: intent.constraints.max_slippage_bps,
        }),
    });

    if let Some(hedge_venue) = hedge_venue.clone() {
        steps.push(PlanStep {
            step_id: format!("{plan_id}:hedge"),
            sequence: (steps.len() + 1) as u32,
            step_type: "adjust_hedge".to_owned(),
            adapter: hedge_venue.clone(),
            approval_required: true,
            idempotency_key: format!("{}:hedge:{}", intent.audit.request_id, hedge_venue),
            failure_policy: StepFailurePolicy {
                mode: FailureMode::Retry,
                retry: RetryPolicy {
                    max_attempts: 2,
                    backoff_ms: 300,
                },
                fallback_venue: None,
            },
            params: PlanStepParams::Hedge(HedgeStepParams {
                instrument: "RELATED-PERP".to_owned(),
                notional_usd: hedge_notional,
                reduce_only: false,
            }),
        });
    }

    Ok(ExecutionPlan {
        plan_id: plan_id.clone(),
        source_kind: "intent".to_owned(),
        source_id: intent.intent_id.clone(),
        request_id: intent.audit.request_id.clone(),
        summary: format!(
            "route {} through {}{}{}",
            intent.intent_id,
            primary_entry,
            if bridge_required {
                " via across bridge"
            } else {
                ""
            },
            hedge_venue
                .as_ref()
                .map(|venue| format!(" with {} hedge", venue))
                .unwrap_or_default(),
        ),
        route: PlannedRoute {
            bridge_venue: bridge_required.then(|| "across".to_owned()),
            entry_venue: primary_entry,
            hedge_venue,
            destination_chain,
            fallback_entry_venue: fallback_entry,
        },
        steps,
        risk_summary: PlanRiskSummary {
            expected_fee_usd: intent.constraints.max_fee_usd.saturating_sub(1),
            expected_slippage_bps: intent.constraints.max_slippage_bps.saturating_sub(5),
            residual_unhedged_exposure_usd: intent
                .objective
                .target_notional_usd
                .saturating_sub(hedge_notional),
        },
    })
}

fn prediction_candidates(
    intent: &CompiledIntent,
    matrix: &CapabilityMatrix,
) -> Result<Vec<String>, PlannerError> {
    let mut supported = Vec::new();
    for venue in &intent.constraints.allowed_venues {
        let Some(capability) = matrix.venue(venue) else {
            return Err(PlannerError::UnsupportedVenue {
                intent_id: intent.intent_id.clone(),
                venue: venue.clone(),
            });
        };
        if matches!(capability.kind, VenueKind::PredictionMarket) {
            supported.push(venue.clone());
        }
    }

    if supported.is_empty() {
        return Err(PlannerError::NoPredictionVenue {
            intent_id: intent.intent_id.clone(),
        });
    }

    supported.sort_by_key(|venue| match venue.as_str() {
        "polymarket" => 0,
        "kalshi" => 1,
        _ => 2,
    });
    Ok(supported)
}

fn hedge_venue(
    intent: &CompiledIntent,
    matrix: &CapabilityMatrix,
) -> Result<Option<String>, PlannerError> {
    let venue = intent
        .post_actions
        .iter()
        .find(|action| action.action_type == "hedge")
        .map(|action| action.venue.clone());
    if let Some(venue) = venue.as_ref() {
        let Some(capability) = matrix.venue(venue) else {
            return Err(PlannerError::UnsupportedHedgeVenue {
                intent_id: intent.intent_id.clone(),
                venue: venue.clone(),
            });
        };
        if !matches!(capability.kind, VenueKind::Hedge) {
            return Err(PlannerError::UnsupportedHedgeVenue {
                intent_id: intent.intent_id.clone(),
                venue: venue.clone(),
            });
        }
    }
    Ok(venue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2ex_compiler::{
        CompileAuditContext, CompiledFunding, CompiledIntentConstraints, CompiledIntentObjective,
        CompiledPostAction,
    };
    use a2ex_control::ExecutionUrgency;

    #[test]
    fn plans_bridge_entry_and_hedge_with_fallback() {
        let plan = plan_intent(&intent(), &CapabilityMatrix::m001_defaults()).expect("plan builds");
        assert_eq!(plan.route.bridge_venue.as_deref(), Some("across"));
        assert_eq!(plan.route.entry_venue, "polymarket");
        assert_eq!(plan.route.fallback_entry_venue.as_deref(), Some("kalshi"));
        assert_eq!(plan.route.hedge_venue.as_deref(), Some("hyperliquid"));
        assert_eq!(plan.steps.len(), 3);
        assert!(matches!(
            plan.steps[1].failure_policy.mode,
            FailureMode::Fallback
        ));
    }

    fn intent() -> CompiledIntent {
        CompiledIntent {
            audit: CompileAuditContext {
                request_id: "req-1".to_owned(),
                source_agent_id: "agent".to_owned(),
                submitted_at: "2026-03-12T00:00:00Z".to_owned(),
                rationale_summary: "test".to_owned(),
                preview_only: false,
                client_request_label: None,
            },
            intent_id: "intent-1".to_owned(),
            intent_type: "open_exposure".to_owned(),
            objective: CompiledIntentObjective {
                domain: "prediction_market".to_owned(),
                target_market: "market-1".to_owned(),
                side: "yes".to_owned(),
                target_notional_usd: 3_000,
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
        }
    }
}
