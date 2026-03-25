use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeStateRecord {
    pub strategy_id: String,
    pub runtime_state: String,
    pub last_transition_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionStateRecord {
    pub execution_id: String,
    pub plan_id: String,
    pub status: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationStateRecord {
    pub execution_id: String,
    pub residual_exposure_usd: i64,
    pub rebalance_required: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalStateSnapshot {
    pub strategies: Vec<StrategyRuntimeStateRecord>,
    pub executions: Vec<ExecutionStateRecord>,
    pub reconciliations: Vec<ReconciliationStateRecord>,
}
