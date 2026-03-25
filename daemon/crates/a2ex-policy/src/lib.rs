use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyInput {
    pub action_id: String,
    pub action_kind: String,
    pub notional_usd: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    AllowWithModifications {
        modifications: BTreeMap<String, Value>,
    },
    Hold {
        reason: String,
    },
    Reject {
        reason: String,
    },
}

pub trait PolicyEvaluator {
    fn evaluate(&self, input: &PolicyInput) -> PolicyDecision;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselinePolicy {
    auto_hold_notional_usd: u64,
}

impl BaselinePolicy {
    pub fn new(auto_hold_notional_usd: u64) -> Self {
        Self {
            auto_hold_notional_usd,
        }
    }

    pub fn auto_hold_notional_usd(&self) -> u64 {
        self.auto_hold_notional_usd
    }
}

impl Default for BaselinePolicy {
    fn default() -> Self {
        Self::new(100_000)
    }
}

impl PolicyEvaluator for BaselinePolicy {
    fn evaluate(&self, input: &PolicyInput) -> PolicyDecision {
        if input.action_kind == "blocked_by_policy" {
            return PolicyDecision::Reject {
                reason: format!(
                    "action {} is blocked by the baseline local policy gate",
                    input.action_kind
                ),
            };
        }

        if input.notional_usd > self.auto_hold_notional_usd {
            return PolicyDecision::Hold {
                reason: format!(
                    "action requires reservation review because notional {} exceeds auto-allow limit {}",
                    input.notional_usd, self.auto_hold_notional_usd
                ),
            };
        }

        PolicyDecision::Allow
    }
}
