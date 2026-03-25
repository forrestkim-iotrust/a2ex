use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRequestKind {
    Intent,
    Strategy,
    Control,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRequestEnvelope<T> {
    pub request_id: String,
    pub request_kind: AgentRequestKind,
    pub source_agent_id: String,
    pub submitted_at: String,
    pub payload: T,
    pub rationale: RationaleSummary,
    pub execution_preferences: ExecutionPreferences,
}

pub type SubmitIntentRequest = AgentRequestEnvelope<Intent>;
pub type RegisterStrategyRequest = AgentRequestEnvelope<Strategy>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RationaleSummary {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub main_risks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPreferences {
    pub preview_only: bool,
    pub allow_fast_path: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    pub intent_id: String,
    pub intent_type: String,
    pub objective: IntentObjective,
    pub constraints: IntentConstraints,
    pub funding: IntentFunding,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_actions: Vec<PostAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentObjective {
    pub domain: String,
    pub target_market: String,
    pub side: String,
    pub target_notional_usd: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentConstraints {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_venues: Vec<String>,
    pub max_slippage_bps: u64,
    pub max_fee_usd: u64,
    pub urgency: ExecutionUrgency,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hedge_ratio_bps: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentFunding {
    pub preferred_asset: String,
    pub source_chain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostAction {
    pub action_type: String,
    pub venue: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strategy {
    pub strategy_id: String,
    pub strategy_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watchers: Vec<WatcherSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger_rules: Vec<TriggerRule>,
    pub calculation_model: CalculationModel,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action_templates: Vec<ActionTemplate>,
    pub constraints: StrategyConstraints,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unwind_rules: Vec<UnwindRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatcherSpec {
    pub watcher_type: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerRule {
    pub trigger_type: String,
    pub metric: String,
    pub operator: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_sec: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalculationModel {
    pub model_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionTemplate {
    pub action_type: String,
    pub venue: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyConstraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_order_usd: Option<u64>,
    pub max_slippage_bps: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rebalances_per_hour: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnwindRule {
    pub condition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionUrgency {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteTarget {
    FastPath,
    PlannedExecution,
    StatefulRuntime,
    Hold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteDecision {
    pub route: RouteTarget,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentAcknowledgement {
    pub request_id: String,
    pub intent_id: String,
    pub request_kind: AgentRequestKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyAcknowledgement {
    pub request_id: String,
    pub strategy_id: String,
    pub request_kind: AgentRequestKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_envelope_round_trips_through_json() {
        let envelope = AgentRequestEnvelope {
            request_id: "req-1".to_owned(),
            request_kind: AgentRequestKind::Intent,
            source_agent_id: "agent-main".to_owned(),
            submitted_at: "2026-03-11T00:00:00Z".to_owned(),
            payload: Intent {
                intent_id: "intent-1".to_owned(),
                intent_type: "open_exposure".to_owned(),
                objective: IntentObjective {
                    domain: "prediction_market".to_owned(),
                    target_market: "us-election-2028".to_owned(),
                    side: "yes".to_owned(),
                    target_notional_usd: 3_000,
                },
                constraints: IntentConstraints {
                    allowed_venues: vec!["polymarket".to_owned(), "kalshi".to_owned()],
                    max_slippage_bps: 80,
                    max_fee_usd: 25,
                    urgency: ExecutionUrgency::Normal,
                    hedge_ratio_bps: Some(4_000),
                },
                funding: IntentFunding {
                    preferred_asset: "USDC".to_owned(),
                    source_chain: "base".to_owned(),
                },
                post_actions: vec![PostAction {
                    action_type: "hedge".to_owned(),
                    venue: "hyperliquid".to_owned(),
                }],
            },
            rationale: RationaleSummary {
                summary: "Opportunity remains positive after costs.".to_owned(),
                main_risks: vec!["spread compression".to_owned()],
            },
            execution_preferences: ExecutionPreferences {
                preview_only: false,
                allow_fast_path: true,
                client_request_label: Some("cli-preview".to_owned()),
            },
        };

        let json = serde_json::to_string(&envelope).expect("serializes");
        let round_trip: AgentRequestEnvelope<Intent> =
            serde_json::from_str(&json).expect("deserializes");

        assert_eq!(round_trip, envelope);
    }
}
