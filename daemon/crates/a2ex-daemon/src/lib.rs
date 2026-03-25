use std::collections::BTreeMap;
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2ex_across_adapter::{AcrossAdapter, AcrossAdapterError, AcrossBridgeQuoteRequest};
use a2ex_compiler::{
    CompiledAgentRequest, CompiledIntent, CompiledStrategy, compile_intent, compile_strategy,
    format_compiler_failure,
};
use a2ex_control::{
    AgentRequestEnvelope, AgentRequestKind, Intent, RouteDecision, RouteTarget, Strategy,
};
use a2ex_evm_adapter::{
    EvmAdapter, EvmAdapterError, NoopEvmAdapter, PreparedEvmTransaction, SignedTransactionBytes,
    TxLifecycleReport, TxLifecycleStatus,
};
use a2ex_fast_path::{
    FastPathPreparationInput, PreparedFastAction, PreparedVenueAction, prepare_fast_action,
    template_from_compiled_intent,
};
use a2ex_gateway::{FastPathRoute, GatewayVerdict, classify as classify_gateway_route};
use a2ex_hyperliquid_adapter::{
    HedgeOrderRequest, HyperliquidAdapter, HyperliquidAdapterError, HyperliquidHedgeSubmitRequest,
    HyperliquidSyncRequest,
};
use a2ex_ipc::{
    DAEMON_CONTROL_METHOD, IpcError, JsonRpcRequest, JsonRpcResponse, LocalControlEndpoint,
    LocalTransport, frame_transport, recv_json_message, send_json_message,
};
use a2ex_planner::{
    CapabilityMatrix, ExecutionPlan, FailureMode, PlanStep, PlanStepParams, plan_intent,
};
use a2ex_policy::{BaselinePolicy, PolicyDecision, PolicyEvaluator, PolicyInput};
use a2ex_prediction_market_adapter::{
    PredictionAuth, PredictionMarketAdapter, PredictionMarketAdapterError, PredictionOrderRequest,
    PredictionVenue,
};
use a2ex_reservation::ReservationManager;
use a2ex_signer_bridge::{
    ApprovalRequest, LocalPeerIdentity, NoopSignerBridge, SignerBridgeError, TxSignRequest,
    ValidatedSignerBridge,
};
use a2ex_state::{
    AUTONOMOUS_RUNTIME_CONTROL_SCOPE, ExecutionAnalyticsRecord, PersistedRuntimeControl,
    StateError, StateRepository, load_execution_analytics, project_execution_analytics,
};
use a2ex_strategy_runtime::{
    MANUAL_STOP_METRIC, MANUAL_STOP_RUNTIME_STATE, RuntimeCommand, RuntimeEvent,
    RuntimePendingHedge, RuntimeTriggerMemory, RuntimeWatcherState, StrategyRuntimeEngine,
    StrategyRuntimeSnapshot, StrategySupervisor, supervisor_interval,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_rusqlite::Connection;
use tracing::Level;
use tracing_subscriber::EnvFilter;

const BOOTSTRAP_PATH: &str = "local-runtime";
const BOOTSTRAP_RECOVERED_AT: &str = "2026-03-11T00:00:00Z";
const DAEMON_SUBMIT_INTENT_METHOD: &str = "daemon.submitIntent";
const DAEMON_REGISTER_STRATEGY_METHOD: &str = "daemon.registerStrategy";
const RUNTIME_SUPERVISOR_TICK_INTERVAL: Duration = Duration::from_millis(50);
const RUNTIME_CONTROL_MODE_ACTIVE: &str = "active";
const RUNTIME_CONTROL_MODE_PAUSED: &str = "paused";
const RUNTIME_CONTROL_MODE_STOPPED: &str = "stopped";
const RUNTIME_REJECTION_CODE_PAUSED: &str = "runtime_paused";
const RUNTIME_REJECTION_CODE_STOPPED: &str = "runtime_stopped";

pub use a2ex_across_adapter::{
    AcrossApproval, AcrossBridgeAck, AcrossBridgeQuote, AcrossBridgeRequest,
};
pub use a2ex_evm_adapter::{
    PreparedEvmTransaction as DaemonPreparedEvmTransaction, ProviderBackedEvmAdapter,
    SimulatedEvmAdapter,
};
pub use a2ex_fast_path::{
    FastActionTemplate as DaemonFastActionTemplate, PreparedFastAction as DaemonPreparedFastAction,
};
pub use a2ex_planner::{
    ExecutionPlan as PlannedExecutionPlan, PlanStep as PlannedExecutionStep, VenueCapability,
};
pub use a2ex_prediction_market_adapter::{PredictionOrderAck, PredictionOrderStatus};
pub use a2ex_reservation::{
    ReservationDecision, ReservationError, ReservationRequest, ReservationState,
    SqliteReservationManager,
};
pub use a2ex_state::{
    CanonicalStateSnapshot, ExecutionStateRecord, JournalEntry, PersistedExecutionPlan,
    PersistedExecutionPlanStep, PersistedPendingHedge, PersistedStrategyRecoverySnapshot,
    PersistedStrategyRegistration, PersistedTriggerMemory, PersistedWatcherState,
    ReconciliationStateRecord, StrategyRuntimeStateRecord,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapSource {
    LocalRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapReport {
    pub source: BootstrapSource,
    pub bootstrap_path: String,
    pub state_db_path: PathBuf,
    pub analytics_db_path: PathBuf,
    pub used_remote_control_plane: bool,
    pub recovered_existing_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    data_dir: PathBuf,
}

impl DaemonConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    pub fn for_data_dir(data_dir: impl AsRef<Path>) -> Self {
        Self::new(data_dir.as_ref().to_path_buf())
    }

    pub fn from_env() -> Result<Self, DaemonError> {
        let data_dir = match env::var_os("A2EX_DAEMON_DATA_DIR") {
            Some(value) => PathBuf::from(value),
            None => env::current_dir()
                .map_err(DaemonError::CurrentDir)?
                .join(".a2ex-daemon"),
        };

        Ok(Self::new(data_dir))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn state_db_path(&self) -> PathBuf {
        self.data_dir.join("state.db")
    }

    pub fn analytics_db_path(&self) -> PathBuf {
        self.data_dir.join("analytics.db")
    }

    pub fn control_socket_path(&self) -> PathBuf {
        self.data_dir.join("control.sock")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub action_id: String,
    pub action_kind: String,
    pub notional_usd: u64,
    pub reservation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedControlRequest {
    pub transport: LocalTransport,
    pub request: ExecutionRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedIntentSubmission {
    pub transport: LocalTransport,
    pub request: AgentRequestEnvelope<Intent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedStrategyRegistration {
    pub transport: LocalTransport,
    pub request: AgentRequestEnvelope<Strategy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSubmissionReceipt {
    pub request_id: String,
    pub intent_id: String,
    pub request_kind: AgentRequestKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRegistrationReceipt {
    pub request_id: String,
    pub strategy_id: String,
    pub request_kind: AgentRequestKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalControlPlane {
    endpoint: LocalControlEndpoint,
}

impl LocalControlPlane {
    pub fn from_config(config: &DaemonConfig) -> Self {
        Self {
            endpoint: LocalControlEndpoint::new(config.control_socket_path()),
        }
    }

    pub fn endpoint(&self) -> &LocalControlEndpoint {
        &self.endpoint
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum AuthorizationVerdict {
    Allow,
    AllowWithModifications {
        modifications: std::collections::BTreeMap<String, serde_json::Value>,
    },
    Hold {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationResult {
    pub action_id: String,
    pub verdict: AuthorizationVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedStepOutcome {
    pub step_id: String,
    pub adapter: String,
    pub status: String,
    pub attempts: u32,
    pub fallback_activated: bool,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedExecutionReport {
    pub plan_id: String,
    pub status: String,
    pub step_outcomes: Vec<PlannedStepOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewApprovalRequirement {
    pub venue: String,
    pub approval_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub required: bool,
    pub auth_summary: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteCapitalSupport {
    pub required_capital_usd: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_capital_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_capital_usd: Option<u64>,
    pub completeness: a2ex_skill_bundle::ProposalQuantitativeCompleteness,
    pub summary: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentPreviewResponse {
    pub request_id: String,
    pub intent_id: String,
    pub route: RouteDecision,
    pub summary: String,
    pub plan_preview: Option<ExecutionPlan>,
    pub capital_support: RouteCapitalSupport,
    pub approval_requirements: Vec<PreviewApprovalRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceReconciliationItem {
    pub step_id: String,
    pub asset: String,
    pub expected_amount_usd: u64,
    pub actual_amount_usd: u64,
    pub delta_usd: i64,
    pub observed_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FillReconciliationItem {
    pub step_id: String,
    pub venue: String,
    pub market: String,
    pub expected_fill_usd: u64,
    pub actual_fill_usd: u64,
    pub delta_usd: i64,
    pub observed_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionReconciliationItem {
    pub step_id: String,
    pub venue: String,
    pub instrument: String,
    pub expected_position_usd: u64,
    pub actual_position_usd: u64,
    pub delta_usd: i64,
    pub observed_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReconciliationReport {
    pub execution_id: String,
    pub plan_id: String,
    pub balances: Vec<BalanceReconciliationItem>,
    pub fills: Vec<FillReconciliationItem>,
    pub positions: Vec<PositionReconciliationItem>,
    pub residual_exposure_usd: i64,
    pub rebalance_required: bool,
    pub reconciled_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionStateQueryResponse {
    pub execution: Option<ExecutionStateRecord>,
    pub route_decision: Option<a2ex_state::PersistedRouteDecision>,
    pub plan: Option<PersistedExecutionPlan>,
    pub steps: Vec<PersistedExecutionPlanStep>,
    pub reconciliation: Option<ExecutionReconciliationReport>,
    pub analytics: Option<ExecutionAnalyticsRecord>,
    pub journal: Vec<JournalEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrategyStateQueryResponse {
    pub strategy: PersistedStrategyRegistration,
    pub route_decision: Option<a2ex_state::PersistedRouteDecision>,
    pub recovery: Option<PersistedStrategyRecoverySnapshot>,
    pub live_hedge_sync: Option<a2ex_hyperliquid_adapter::HyperliquidSyncSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeActionProjection {
    pub kind: String,
    pub status: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeOutcomeProjection {
    pub code: String,
    pub message: String,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeMonitoringProjection {
    pub current_phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_action: Option<StrategyRuntimeActionProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_intended_action: Option<StrategyRuntimeActionProjection>,
    #[serde(default)]
    pub last_runtime_failure: Option<StrategyRuntimeOutcomeProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeReconciliationProjection {
    pub status: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_exposure_usd: Option<i64>,
    pub rebalance_required: bool,
    pub owner_action_needed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanRequestSupportResponse {
    pub request_id: String,
    pub request_kind: String,
    pub route: Option<RouteDecision>,
    pub rationale_summary: String,
    pub main_risks: Vec<String>,
    pub capital_required_usd: u64,
    pub capital_support: RouteCapitalSupport,
    pub execution_status: Option<String>,
    pub approvals_needed: Vec<PreviewApprovalRequirement>,
    pub justification_facts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSupportTruth {
    pub capital_support: RouteCapitalSupport,
    pub approval_requirements: Vec<PreviewApprovalRequirement>,
}

pub trait SignerHandoff: Send + Sync {
    fn handoff(&self, request: &ExecutionRequest);
}

struct RuntimeSupervisorHandle {
    event_tx: mpsc::Sender<RuntimeEvent>,
    _tick_task: JoinHandle<()>,
    _output_task: JoinHandle<()>,
    _supervisor_task: JoinHandle<()>,
}

#[derive(Debug, Default)]
pub struct NoopRuntimeSigner;

impl SignerHandoff for NoopRuntimeSigner {
    fn handoff(&self, _request: &ExecutionRequest) {}
}

pub struct DaemonService<P, R, S, B = NoopSignerBridge, A = NoopEvmAdapter> {
    analytics_db_path: PathBuf,
    state_db_path: PathBuf,
    control_plane: LocalControlPlane,
    recorded_intents: Arc<Mutex<Vec<AgentRequestEnvelope<Intent>>>>,
    recorded_strategies: Arc<Mutex<Vec<AgentRequestEnvelope<Strategy>>>>,
    compiled_intents: Arc<Mutex<Vec<CompiledIntent>>>,
    compiled_strategies: Arc<Mutex<Vec<CompiledStrategy>>>,
    policy: P,
    reservations: Arc<R>,
    signer: Arc<S>,
    signer_bridge: Arc<B>,
    evm_adapter: Arc<A>,
    across_adapter: AcrossAdapter,
    prediction_market_adapter: PredictionMarketAdapter,
    hyperliquid_adapter: HyperliquidAdapter,
    capability_matrix: CapabilityMatrix,
    runtime_registry: Arc<Mutex<BTreeMap<String, RuntimeSupervisorHandle>>>,
    runtime_commands: Arc<Mutex<BTreeMap<String, Vec<RuntimeCommand>>>>,
}

impl<P, R, S> DaemonService<P, R, S, NoopSignerBridge, NoopEvmAdapter>
where
    P: PolicyEvaluator,
    R: ReservationManager,
    S: SignerHandoff,
{
    pub fn from_config(config: &DaemonConfig, policy: P, reservations: R, signer: Arc<S>) -> Self {
        Self {
            analytics_db_path: config.analytics_db_path(),
            state_db_path: config.state_db_path(),
            control_plane: LocalControlPlane::from_config(config),
            recorded_intents: Arc::new(Mutex::new(Vec::new())),
            recorded_strategies: Arc::new(Mutex::new(Vec::new())),
            compiled_intents: Arc::new(Mutex::new(Vec::new())),
            compiled_strategies: Arc::new(Mutex::new(Vec::new())),
            policy,
            reservations: Arc::new(reservations),
            signer,
            signer_bridge: Arc::new(NoopSignerBridge),
            evm_adapter: Arc::new(NoopEvmAdapter),
            across_adapter: AcrossAdapter::default(),
            prediction_market_adapter: PredictionMarketAdapter::default(),
            hyperliquid_adapter: HyperliquidAdapter::default(),
            capability_matrix: CapabilityMatrix::m001_defaults(),
            runtime_registry: Arc::new(Mutex::new(BTreeMap::new())),
            runtime_commands: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl<P, R, S, B> DaemonService<P, R, S, B, NoopEvmAdapter>
where
    P: PolicyEvaluator,
    R: ReservationManager,
    S: SignerHandoff,
    B: ValidatedSignerBridge,
{
    pub fn from_config_with_signer_bridge(
        config: &DaemonConfig,
        policy: P,
        reservations: R,
        signer: Arc<S>,
        signer_bridge: B,
    ) -> Self {
        Self {
            analytics_db_path: config.analytics_db_path(),
            state_db_path: config.state_db_path(),
            control_plane: LocalControlPlane::from_config(config),
            recorded_intents: Arc::new(Mutex::new(Vec::new())),
            recorded_strategies: Arc::new(Mutex::new(Vec::new())),
            compiled_intents: Arc::new(Mutex::new(Vec::new())),
            compiled_strategies: Arc::new(Mutex::new(Vec::new())),
            policy,
            reservations: Arc::new(reservations),
            signer,
            signer_bridge: Arc::new(signer_bridge),
            evm_adapter: Arc::new(NoopEvmAdapter),
            across_adapter: AcrossAdapter::default(),
            prediction_market_adapter: PredictionMarketAdapter::default(),
            hyperliquid_adapter: HyperliquidAdapter::default(),
            capability_matrix: CapabilityMatrix::m001_defaults(),
            runtime_registry: Arc::new(Mutex::new(BTreeMap::new())),
            runtime_commands: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl<P, R, S, B, A> DaemonService<P, R, S, B, A>
where
    P: PolicyEvaluator,
    R: ReservationManager,
    S: SignerHandoff,
    B: ValidatedSignerBridge,
    A: EvmAdapter,
{
    pub fn from_config_with_fast_path(
        config: &DaemonConfig,
        policy: P,
        reservations: R,
        signer: Arc<S>,
        signer_bridge: B,
        evm_adapter: A,
    ) -> Self {
        Self {
            analytics_db_path: config.analytics_db_path(),
            state_db_path: config.state_db_path(),
            control_plane: LocalControlPlane::from_config(config),
            recorded_intents: Arc::new(Mutex::new(Vec::new())),
            recorded_strategies: Arc::new(Mutex::new(Vec::new())),
            compiled_intents: Arc::new(Mutex::new(Vec::new())),
            compiled_strategies: Arc::new(Mutex::new(Vec::new())),
            policy,
            reservations: Arc::new(reservations),
            signer,
            signer_bridge: Arc::new(signer_bridge),
            evm_adapter: Arc::new(evm_adapter),
            across_adapter: AcrossAdapter::default(),
            prediction_market_adapter: PredictionMarketAdapter::default(),
            hyperliquid_adapter: HyperliquidAdapter::default(),
            capability_matrix: CapabilityMatrix::m001_defaults(),
            runtime_registry: Arc::new(Mutex::new(BTreeMap::new())),
            runtime_commands: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn from_config_with_fast_path_and_hedge_adapter(
        config: &DaemonConfig,
        policy: P,
        reservations: R,
        signer: Arc<S>,
        signer_bridge: B,
        evm_adapter: A,
        hyperliquid_adapter: HyperliquidAdapter,
    ) -> Self {
        Self::from_config_with_multi_venue_adapters(
            config,
            policy,
            reservations,
            signer,
            signer_bridge,
            evm_adapter,
            AcrossAdapter::default(),
            PredictionMarketAdapter::default(),
            hyperliquid_adapter,
        )
    }

    pub fn from_config_with_multi_venue_adapters(
        config: &DaemonConfig,
        policy: P,
        reservations: R,
        signer: Arc<S>,
        signer_bridge: B,
        evm_adapter: A,
        across_adapter: AcrossAdapter,
        prediction_market_adapter: PredictionMarketAdapter,
        hyperliquid_adapter: HyperliquidAdapter,
    ) -> Self {
        Self {
            analytics_db_path: config.analytics_db_path(),
            state_db_path: config.state_db_path(),
            control_plane: LocalControlPlane::from_config(config),
            recorded_intents: Arc::new(Mutex::new(Vec::new())),
            recorded_strategies: Arc::new(Mutex::new(Vec::new())),
            compiled_intents: Arc::new(Mutex::new(Vec::new())),
            compiled_strategies: Arc::new(Mutex::new(Vec::new())),
            policy,
            reservations: Arc::new(reservations),
            signer,
            signer_bridge: Arc::new(signer_bridge),
            evm_adapter: Arc::new(evm_adapter),
            across_adapter,
            prediction_market_adapter,
            hyperliquid_adapter,
            capability_matrix: CapabilityMatrix::m001_defaults(),
            runtime_registry: Arc::new(Mutex::new(BTreeMap::new())),
            runtime_commands: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn control_plane(&self) -> &LocalControlPlane {
        &self.control_plane
    }

    pub fn active_runtime_supervisors(&self) -> Vec<String> {
        self.runtime_registry
            .lock()
            .expect("runtime registry lock")
            .keys()
            .cloned()
            .collect()
    }

    pub fn take_runtime_commands(&self, strategy_id: &str) -> Vec<RuntimeCommand> {
        self.runtime_commands
            .lock()
            .expect("runtime command lock")
            .remove(strategy_id)
            .unwrap_or_default()
    }

    pub async fn publish_runtime_watcher_sample(
        &self,
        strategy_id: &str,
        sample: RuntimeWatcherState,
    ) -> Result<(), DaemonError> {
        self.publish_runtime_event(strategy_id, RuntimeEvent::WatcherSample(sample))
            .await
    }

    async fn publish_runtime_event(
        &self,
        strategy_id: &str,
        event: RuntimeEvent,
    ) -> Result<(), DaemonError> {
        let event_tx = self
            .runtime_registry
            .lock()
            .expect("runtime registry lock")
            .get(strategy_id)
            .map(|handle| handle.event_tx.clone())
            .ok_or_else(|| DaemonError::FastPathPreparation {
                reason: format!("strategy {strategy_id} has no active runtime supervisor"),
            })?;
        event_tx
            .send(event)
            .await
            .map_err(|_| DaemonError::FastPathPreparation {
                reason: format!("strategy {strategy_id} runtime supervisor is unavailable"),
            })
    }

    fn spawn_runtime_supervisor(&self, strategy_id: &str, snapshot: StrategyRuntimeSnapshot) {
        let (event_tx, event_rx) = mpsc::channel(16);
        let (output_tx, mut output_rx) = mpsc::channel(16);
        let strategy_id_for_supervisor = strategy_id.to_owned();
        let supervisor_task = tokio::spawn(async move {
            if let Err(error) = StrategySupervisor::new(snapshot)
                .run(event_rx, output_tx)
                .await
            {
                tracing::warn!(strategy_id = %strategy_id_for_supervisor, error = %error, "runtime supervisor exited");
            }
        });

        let state_db_path = self.state_db_path.clone();
        let strategy_id_for_output = strategy_id.to_owned();
        let runtime_commands = Arc::clone(&self.runtime_commands);
        let hyperliquid_adapter = self.hyperliquid_adapter.clone();
        let output_task = tokio::spawn(async move {
            while let Some(output) = output_rx.recv().await {
                if matches!(output.event, RuntimeEvent::Tick { .. })
                    && snapshot_metric_bool(&output.snapshot.metrics, "venue_sync_required")
                {
                    continue;
                }
                let strategy_id = strategy_id_for_output.clone();
                let repository = match StateRepository::open(&state_db_path).await {
                    Ok(repository) => repository,
                    Err(error) => {
                        tracing::warn!(strategy_id = %strategy_id, error = %error, "failed to open state repository for runtime output");
                        continue;
                    }
                };
                let persisted = match repository
                    .load_strategy_recovery_snapshot(&strategy_id)
                    .await
                {
                    Ok(Some(snapshot)) => {
                        persisted_runtime_snapshot(snapshot.strategy, output.snapshot.clone())
                    }
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::warn!(strategy_id = %strategy_id, error = %error, "failed to load strategy snapshot for runtime output");
                        continue;
                    }
                };
                if let Err(error) = repository
                    .persist_strategy_recovery_snapshot(&persisted)
                    .await
                {
                    tracing::warn!(strategy_id = %strategy_id, error = %error, "failed to persist runtime output snapshot");
                    continue;
                }
                if !output.commands.is_empty() {
                    runtime_commands
                        .lock()
                        .expect("runtime command lock")
                        .entry(strategy_id.clone())
                        .or_default()
                        .extend(output.commands.clone());
                }
                if matches!(output.event, RuntimeEvent::WatcherSample(_))
                    && snapshot_metric_bool(&output.snapshot.metrics, "venue_sync_required")
                {
                    let synced_at = runtime_event_timestamp(&output.event);
                    if let Err(error) = sync_strategy_hedge_with_adapter(
                        &state_db_path,
                        &hyperliquid_adapter,
                        &strategy_id,
                        &synced_at,
                    )
                    .await
                    {
                        tracing::warn!(strategy_id = %strategy_id, error = %error, "automatic runtime hedge sync failed");
                    } else if let Err(error) = continue_runtime_after_sync(
                        &state_db_path,
                        &strategy_id,
                        output.snapshot.watcher_states.clone(),
                        &synced_at,
                        &runtime_commands,
                    )
                    .await
                    {
                        tracing::warn!(strategy_id = %strategy_id, error = %error, "automatic runtime continuation failed");
                    }
                }
            }
        });

        let strategy_id_for_tick = strategy_id.to_owned();
        let tick_event_tx = event_tx.clone();
        let tick_task = tokio::spawn(async move {
            let mut interval = supervisor_interval(RUNTIME_SUPERVISOR_TICK_INTERVAL);
            loop {
                interval.tick().await;
                let now = match current_runtime_timestamp() {
                    Some(now) => now,
                    None => continue,
                };
                if tick_event_tx
                    .send(RuntimeEvent::Tick { now })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            tracing::debug!(strategy_id = %strategy_id_for_tick, "runtime tick loop exited");
        });

        self.runtime_registry
            .lock()
            .expect("runtime registry lock")
            .insert(
                strategy_id.to_owned(),
                RuntimeSupervisorHandle {
                    event_tx,
                    _tick_task: tick_task,
                    _output_task: output_task,
                    _supervisor_task: supervisor_task,
                },
            );
    }

    pub async fn submit_intent(
        &self,
        request: JsonRpcRequest<serde_json::Value>,
    ) -> Result<JsonRpcResponse<IntentSubmissionReceipt>, DaemonError> {
        let request_id = request.id.clone();
        let decoded = match decode_intent_submission(self.control_plane.endpoint(), request) {
            Ok(decoded) => decoded,
            Err(DaemonError::InvalidIntentEnvelope { source }) => {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_020,
                    format!("invalid intent payload: {source}"),
                ));
            }
            Err(DaemonError::UnexpectedAgentRequestKind { kind }) => {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_021,
                    format!("intent submission requires request_kind=intent, got {kind}"),
                ));
            }
            Err(error) => return Err(error),
        };

        self.recorded_intents
            .lock()
            .expect("recorded intent store lock")
            .push(decoded.request.clone());

        let repository = StateRepository::open(&self.state_db_path).await?;
        repository
            .persist_intent_submission(&decoded.request)
            .await?;

        let compiled = match compile_intent(&decoded.request) {
            Ok(compiled) => compiled,
            Err(error) => {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_022,
                    format_compiler_failure(&error),
                ));
            }
        };
        let route_decision =
            classify_gateway_route(&CompiledAgentRequest::Intent(compiled.clone()))
                .route_decision();
        repository
            .persist_route_decision(
                &decoded.request.request_id,
                "intent",
                &decoded.request.payload.intent_id,
                &route_decision,
                &decoded.request.submitted_at,
            )
            .await?;
        self.compiled_intents
            .lock()
            .expect("compiled intent store lock")
            .push(compiled);

        Ok(JsonRpcResponse::success(
            request_id,
            IntentSubmissionReceipt {
                request_id: decoded.request.request_id,
                intent_id: decoded.request.payload.intent_id,
                request_kind: decoded.request.request_kind,
            },
        ))
    }

    pub fn recorded_intents(&self) -> Vec<AgentRequestEnvelope<Intent>> {
        self.recorded_intents
            .lock()
            .expect("recorded intent store lock")
            .clone()
    }

    pub fn compiled_intents(&self) -> Vec<CompiledIntent> {
        self.compiled_intents
            .lock()
            .expect("compiled intent store lock")
            .clone()
    }

    pub fn capability_matrix(&self) -> CapabilityMatrix {
        self.capability_matrix.clone()
    }

    pub async fn preview_intent_request(
        &self,
        request_id: &str,
    ) -> Result<IntentPreviewResponse, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let route_decision = repository
            .load_route_decision(request_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("request {request_id} has no persisted route decision"),
            })?;
        let envelope = repository
            .load_intent_envelope(request_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("request {request_id} has no persisted intent envelope"),
            })?;
        let compiled = compile_intent(&envelope).map_err(|error| DaemonError::PlanExecution {
            reason: format!(
                "request {request_id} could not be recompiled for preview: {}",
                format_compiler_failure(&error)
            ),
        })?;
        let plan_preview = match route_decision.route.route {
            RouteTarget::PlannedExecution => Some(plan_intent(&compiled, &self.capability_matrix)?),
            _ => None,
        };
        let support_truth = project_route_support_truth(
            &self.capability_matrix,
            &compiled,
            plan_preview.as_ref(),
            None,
        );

        Ok(IntentPreviewResponse {
            request_id: request_id.to_owned(),
            intent_id: compiled.intent_id,
            route: route_decision.route.clone(),
            summary: route_decision.route.summary.clone(),
            plan_preview,
            capital_support: support_truth.capital_support,
            approval_requirements: support_truth.approval_requirements,
        })
    }

    pub async fn query_execution_state(
        &self,
        execution_id: &str,
    ) -> Result<ExecutionStateQueryResponse, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let snapshot = repository.load_snapshot().await?;
        let execution = snapshot
            .executions
            .into_iter()
            .find(|record| record.execution_id == execution_id);
        let plan = repository.load_execution_plan(execution_id).await?;
        let route_decision = if let Some(plan) = &plan {
            repository.load_route_decision(&plan.request_id).await?
        } else {
            None
        };
        let steps = if plan.is_some() {
            repository.load_execution_plan_steps(execution_id).await?
        } else {
            Vec::new()
        };
        let persisted_reconciliation = repository.load_reconciliation_state(execution_id).await?;
        let journal = repository
            .load_journal()
            .await?
            .into_iter()
            .filter(|entry| entry.stream_id == execution_id)
            .collect();
        let analytics = load_execution_analytics(&self.analytics_db_path)
            .await
            .ok()
            .and_then(|rows| {
                rows.into_iter()
                    .find(|row| row.execution_id == execution_id)
            });
        let reconciliation = match (plan.as_ref(), persisted_reconciliation) {
            (Some(plan), Some(record)) => {
                let mut report = reconcile_plan_execution(plan, &steps, &record.updated_at);
                report.residual_exposure_usd = record.residual_exposure_usd;
                report.rebalance_required = record.rebalance_required;
                Some(report)
            }
            _ => None,
        };

        Ok(ExecutionStateQueryResponse {
            execution,
            route_decision,
            plan,
            steps,
            reconciliation,
            analytics,
            journal,
        })
    }

    pub async fn query_strategy_state(
        &self,
        strategy_id: &str,
    ) -> Result<StrategyStateQueryResponse, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let strategy = repository
            .load_strategy_registration(strategy_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("strategy {strategy_id} is not registered"),
            })?;
        let route_decision = repository.load_route_decision(&strategy.request_id).await?;
        let recovery = repository
            .load_strategy_recovery_snapshot(strategy_id)
            .await?;
        let live_hedge_sync = if let Some(snapshot) = recovery.as_ref() {
            if let Some(hedge) = snapshot.pending_hedge.as_ref() {
                Some(
                    self.hyperliquid_adapter
                        .sync_state(HyperliquidSyncRequest {
                            signer_address: hedge.signer_address.clone(),
                            account_address: hedge.account_address.clone(),
                            order_id: hedge.order_id,
                            aggregate_fills: true,
                        })
                        .await?,
                )
            } else {
                None
            }
        } else {
            None
        };

        Ok(StrategyStateQueryResponse {
            strategy,
            route_decision,
            recovery,
            live_hedge_sync,
        })
    }

    pub async fn human_request_support(
        &self,
        request_id: &str,
    ) -> Result<HumanRequestSupportResponse, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let route_decision = repository.load_route_decision(request_id).await?;
        let envelope = repository
            .load_intent_envelope(request_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("request {request_id} has no persisted intent envelope"),
            })?;
        let compiled = compile_intent(&envelope).map_err(|error| DaemonError::PlanExecution {
            reason: format!(
                "request {request_id} could not be recompiled for human support: {}",
                format_compiler_failure(&error)
            ),
        })?;
        let plan_preview = match route_decision
            .as_ref()
            .map(|decision| &decision.route.route)
        {
            Some(RouteTarget::PlannedExecution) => {
                Some(plan_intent(&compiled, &self.capability_matrix)?)
            }
            _ => None,
        };
        let reserved_capital_usd = repository
            .load_capital_reservations_for_execution(request_id)
            .await?
            .into_iter()
            .filter(|reservation| reservation.state == "held")
            .map(|reservation| reservation.amount)
            .sum::<u64>();
        let support_truth = project_route_support_truth(
            &self.capability_matrix,
            &compiled,
            plan_preview.as_ref(),
            (reserved_capital_usd > 0).then_some(reserved_capital_usd),
        );
        let approvals_needed = support_truth.approval_requirements.clone();
        let execution_status = if let Some(plan) = plan_preview.as_ref() {
            repository
                .load_execution_plan(&plan.plan_id)
                .await?
                .map(|persisted| persisted.status)
        } else {
            None
        };
        let mut justification_facts = vec![format!(
            "Target notional is ${} on market {}.",
            compiled.objective.target_notional_usd, compiled.objective.target_market
        )];
        if let Some(route) = route_decision.as_ref() {
            justification_facts.push(format!("Route selected: {}.", route.route.summary));
        }
        if let Some(plan) = plan_preview.as_ref() {
            justification_facts.push(format!(
                "Planned steps: {}.",
                plan.steps
                    .iter()
                    .map(|step| format!("{} via {}", step.step_type, step.adapter))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        justification_facts.push(format!(
            "Capital readiness: {} {}",
            support_truth.capital_support.summary, support_truth.capital_support.reason
        ));
        if !approvals_needed.is_empty() {
            justification_facts.push(format!(
                "Local approvals required: {}.",
                approvals_needed
                    .iter()
                    .map(|approval| {
                        let mut detail = format!("{} ({})", approval.venue, approval.approval_type);
                        if let Some(context) = approval.context.as_deref() {
                            detail.push_str(&format!(" via {context}"));
                        }
                        detail
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        Ok(HumanRequestSupportResponse {
            request_id: request_id.to_owned(),
            request_kind: "intent".to_owned(),
            route: route_decision.map(|decision| decision.route),
            rationale_summary: envelope.rationale.summary,
            main_risks: envelope.rationale.main_risks,
            capital_required_usd: compiled.objective.target_notional_usd,
            capital_support: support_truth.capital_support,
            execution_status,
            approvals_needed,
            justification_facts,
        })
    }

    pub async fn reconcile_execution(
        &self,
        execution_id: &str,
        reconciled_at: &str,
    ) -> Result<ExecutionReconciliationReport, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let plan = repository
            .load_execution_plan(execution_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("execution {execution_id} has no persisted plan"),
            })?;
        let steps = repository.load_execution_plan_steps(execution_id).await?;
        let report = reconcile_plan_execution(&plan, &steps, reconciled_at);
        repository
            .persist_reconciliation_state(
                execution_id,
                report.residual_exposure_usd,
                report.rebalance_required,
                reconciled_at,
            )
            .await?;
        Ok(report)
    }

    pub async fn plan_intent_request(
        &self,
        request_id: &str,
    ) -> Result<ExecutionPlan, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let route_decision = repository
            .load_route_decision(request_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("request {request_id} has no persisted route decision"),
            })?;
        if route_decision.route.route != RouteTarget::PlannedExecution {
            return Err(DaemonError::PlanExecution {
                reason: format!(
                    "request {request_id} is routed as {:?}, not planned_execution",
                    route_decision.route.route
                ),
            });
        }

        let envelope = repository
            .load_intent_envelope(request_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("request {request_id} has no persisted intent envelope"),
            })?;
        let compiled = compile_intent(&envelope).map_err(|error| DaemonError::PlanExecution {
            reason: format!(
                "request {request_id} could not be recompiled for planning: {}",
                format_compiler_failure(&error)
            ),
        })?;
        let plan = plan_intent(&compiled, &self.capability_matrix)?;
        repository
            .persist_execution_plan(&plan, "planned", &compiled.audit.submitted_at)
            .await?;
        for step in &plan.steps {
            repository
                .persist_execution_plan_step(&seed_plan_step(
                    &plan.plan_id,
                    step,
                    "pending",
                    0,
                    None,
                    None,
                    &compiled.audit.submitted_at,
                ))
                .await?;
        }
        Ok(plan)
    }

    pub async fn execute_planned_intent(
        &self,
        plan_id: &str,
        signer_peer: LocalPeerIdentity,
        started_at: &str,
    ) -> Result<PlannedExecutionReport, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let persisted_plan = repository
            .load_execution_plan(plan_id)
            .await?
            .ok_or_else(|| DaemonError::PlanExecution {
                reason: format!("plan {plan_id} is not persisted"),
            })?;
        let existing_steps = repository.load_execution_plan_steps(plan_id).await?;
        repository
            .persist_execution_state(plan_id, plan_id, "executing")
            .await?;

        let mut step_outcomes = Vec::new();
        for step in &persisted_plan.plan.steps {
            if let Some(existing) = existing_steps
                .iter()
                .find(|candidate| candidate.step_id == step.step_id)
                && is_terminal_step_status(&existing.status)
            {
                step_outcomes.push(PlannedStepOutcome {
                    step_id: existing.step_id.clone(),
                    adapter: existing.adapter.clone(),
                    status: existing.status.clone(),
                    attempts: existing.attempts,
                    fallback_activated: existing
                        .metadata_json
                        .as_deref()
                        .is_some_and(|json| json.contains("fallback_venue")),
                    metadata: existing
                        .metadata_json
                        .as_deref()
                        .and_then(|json| serde_json::from_str(json).ok())
                        .unwrap_or_else(|| serde_json::json!({"replayed": true})),
                });
                continue;
            }

            let outcome = self
                .execute_plan_step(step, &persisted_plan.plan, &signer_peer, started_at)
                .await?;
            step_outcomes.push(outcome.clone());
            if !is_terminal_step_status(&outcome.status) {
                repository
                    .persist_execution_state(plan_id, plan_id, "failed")
                    .await?;
                return Ok(PlannedExecutionReport {
                    plan_id: plan_id.to_owned(),
                    status: "failed".to_owned(),
                    step_outcomes,
                });
            }
        }

        repository
            .persist_execution_state(plan_id, plan_id, "completed")
            .await?;
        repository
            .persist_execution_plan(&persisted_plan.plan, "completed", started_at)
            .await?;
        let _ = project_execution_analytics(&self.state_db_path, &self.analytics_db_path).await;

        Ok(PlannedExecutionReport {
            plan_id: plan_id.to_owned(),
            status: "completed".to_owned(),
            step_outcomes,
        })
    }

    async fn execute_plan_step(
        &self,
        step: &PlanStep,
        plan: &ExecutionPlan,
        signer_peer: &LocalPeerIdentity,
        started_at: &str,
    ) -> Result<PlannedStepOutcome, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let existing = repository
            .load_execution_plan_steps(&plan.plan_id)
            .await?
            .into_iter()
            .find(|candidate| candidate.step_id == step.step_id);
        let mut attempts = existing.as_ref().map_or(0, |step| step.attempts);
        let mut active_adapter = step.adapter.clone();
        let mut fallback_activated = false;

        loop {
            attempts += 1;
            let mut metadata = serde_json::json!({"adapter": active_adapter, "attempt": attempts});
            let result = match &step.params {
                PlanStepParams::Bridge(params) => {
                    self.request_step_approval(step, signer_peer, params.amount_usd, started_at)
                        .await?;
                    self.reservations
                        .require_held(&plan.request_id, params.amount_usd)
                        .await?;
                    let result = self
                        .across_adapter
                        .bridge_asset(
                            "across-signer",
                            "destination-wallet",
                            AcrossBridgeQuoteRequest {
                                asset: params.asset.clone(),
                                amount_usd: params.amount_usd,
                                source_chain: params.source_chain.clone(),
                                destination_chain: params.destination_chain.clone(),
                                depositor: None,
                                recipient: None,
                                output_token: None,
                            },
                        )
                        .await
                        .map(|(ack, status)| {
                            metadata = serde_json::json!({
                                "deposit_id": ack.deposit_id,
                                "route_id": ack.route_id,
                                "bridge_status": status.status.clone(),
                                "destination_tx_id": status.destination_tx_id,
                            });
                            status.status
                        })
                        .map_err(DaemonError::AcrossAdapter);
                    result_with_fallback_status(result, "settled")
                }
                PlanStepParams::Entry(params) => {
                    self.request_step_approval(step, signer_peer, params.notional_usd, started_at)
                        .await?;
                    self.reservations
                        .require_held(&plan.request_id, params.notional_usd)
                        .await?;
                    let venue = prediction_venue(&active_adapter)?;
                    let result = self
                        .prediction_market_adapter
                        .place_and_sync(PredictionOrderRequest {
                            venue,
                            market: params.market.clone(),
                            side: params.side.clone(),
                            size: params.notional_usd.to_string(),
                            price: "1".to_owned(),
                            max_fee_bps: params.max_fee_usd,
                            max_slippage_bps: params.max_slippage_bps,
                            idempotency_key: step.idempotency_key.clone(),
                            auth: prediction_auth(venue),
                        })
                        .await
                        .map(|(ack, status)| {
                            metadata = serde_json::json!({
                                "venue": ack.venue.as_str(),
                                "order_id": ack.order_id,
                                "order_status": status.status.clone(),
                                "filled_amount": status.filled_amount,
                            });
                            status.status
                        })
                        .map_err(DaemonError::PredictionMarketAdapter);
                    result_with_fallback_status(result, "filled")
                }
                PlanStepParams::Hedge(params) => {
                    self.request_step_approval(step, signer_peer, params.notional_usd, started_at)
                        .await?;
                    self.reservations
                        .require_held(&plan.request_id, params.notional_usd)
                        .await?;
                    let prepared = self.hyperliquid_adapter.prepare_order(
                        None,
                        HedgeOrderRequest {
                            strategy_id: plan.source_id.clone(),
                            instrument: params.instrument.clone(),
                            notional_usd: params.notional_usd,
                            reduce_only: params.reduce_only,
                        },
                    );
                    let ack = self
                        .hyperliquid_adapter
                        .place_hedge_order(HyperliquidHedgeSubmitRequest {
                            prepared: prepared.clone(),
                            signer_address: "hl-plan-signer".to_owned(),
                            account_address: "hl-plan-account".to_owned(),
                            asset: 0,
                            is_buy: !params.reduce_only,
                            price: params.notional_usd.to_string(),
                            size: "1.0".to_owned(),
                            time_in_force: "Ioc".to_owned(),
                        })
                        .await?;
                    let sync = self
                        .hyperliquid_adapter
                        .sync_state(HyperliquidSyncRequest {
                            signer_address: ack.signer_address.clone(),
                            account_address: ack.account_address.clone(),
                            order_id: ack.order_id,
                            aggregate_fills: true,
                        })
                        .await?;
                    metadata = serde_json::json!({
                        "client_order_id": prepared.client_order_id,
                        "order_id": ack.order_id,
                        "order_status": sync.order_status.as_ref().map(|status| status.status.clone()),
                        "positions": sync.positions,
                    });
                    Ok(synced_status(&step.step_id, &ack.status, &sync))
                }
            };

            match result {
                Ok(status) if is_terminal_step_status(&status) => {
                    repository
                        .persist_execution_plan_step(&seed_plan_step(
                            &plan.plan_id,
                            step,
                            &status,
                            attempts,
                            None,
                            Some(metadata.to_string()),
                            started_at,
                        ))
                        .await?;
                    self.reservations.release(&plan.request_id, None).await.ok();
                    return Ok(PlannedStepOutcome {
                        step_id: step.step_id.clone(),
                        adapter: active_adapter,
                        status,
                        attempts,
                        fallback_activated,
                        metadata,
                    });
                }
                Ok(status) => {
                    repository
                        .persist_execution_plan_step(&seed_plan_step(
                            &plan.plan_id,
                            step,
                            &status,
                            attempts,
                            Some(format!("step ended in non-terminal status {status}")),
                            Some(metadata.to_string()),
                            started_at,
                        ))
                        .await?;
                    return Ok(PlannedStepOutcome {
                        step_id: step.step_id.clone(),
                        adapter: active_adapter,
                        status,
                        attempts,
                        fallback_activated,
                        metadata,
                    });
                }
                Err(error) if attempts < u32::from(step.failure_policy.retry.max_attempts) => {
                    repository
                        .persist_execution_plan_step(&seed_plan_step(
                            &plan.plan_id,
                            step,
                            "retrying",
                            attempts,
                            Some(error.to_string()),
                            Some(metadata.to_string()),
                            started_at,
                        ))
                        .await?;
                    continue;
                }
                Err(error)
                    if matches!(step.failure_policy.mode, FailureMode::Fallback)
                        && step.failure_policy.fallback_venue.is_some()
                        && !fallback_activated =>
                {
                    fallback_activated = true;
                    active_adapter = step
                        .failure_policy
                        .fallback_venue
                        .clone()
                        .expect("fallback venue");
                    metadata["fallback_venue"] = serde_json::Value::String(active_adapter.clone());
                    repository
                        .persist_execution_plan_step(&seed_plan_step(
                            &plan.plan_id,
                            step,
                            "fallback_routed",
                            attempts,
                            Some(error.to_string()),
                            Some(metadata.to_string()),
                            started_at,
                        ))
                        .await?;
                    continue;
                }
                Err(error) => {
                    repository
                        .persist_execution_plan_step(&seed_plan_step(
                            &plan.plan_id,
                            step,
                            "failed",
                            attempts,
                            Some(error.to_string()),
                            Some(metadata.to_string()),
                            started_at,
                        ))
                        .await?;
                    return Ok(PlannedStepOutcome {
                        step_id: step.step_id.clone(),
                        adapter: active_adapter,
                        status: "failed".to_owned(),
                        attempts,
                        fallback_activated,
                        metadata: serde_json::json!({"error": error.to_string()}),
                    });
                }
            }
        }
    }

    async fn request_step_approval(
        &self,
        step: &PlanStep,
        signer_peer: &LocalPeerIdentity,
        notional_usd: u64,
        started_at: &str,
    ) -> Result<(), DaemonError> {
        if !step.approval_required {
            return Ok(());
        }
        self.signer_bridge
            .request_approval_from_peer(
                signer_peer,
                ApprovalRequest {
                    action_id: step.idempotency_key.clone(),
                    action_kind: step.step_type.clone(),
                    reservation_id: step.idempotency_key.clone(),
                    notional_usd,
                    origin_transport: self.control_plane.endpoint().transport(),
                },
            )
            .await?;
        self.signer.handoff(&ExecutionRequest {
            action_id: step.idempotency_key.clone(),
            action_kind: step.step_type.clone(),
            notional_usd,
            reservation_id: format!("{}:{}", step.step_id, started_at),
        });
        Ok(())
    }

    pub async fn prepare_fast_path_action(
        &self,
        request_id: &str,
        reservation_id: &str,
    ) -> Result<PreparedFastAction, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let route_decision = repository
            .load_route_decision(request_id)
            .await?
            .ok_or_else(|| DaemonError::FastPathPreparation {
                reason: format!("request {request_id} has no persisted route decision"),
            })?;

        if route_decision.route.route != RouteTarget::FastPath {
            return Err(DaemonError::FastPathPreparation {
                reason: format!(
                    "request {request_id} is routed as {:?}, not fast_path",
                    route_decision.route.route
                ),
            });
        }

        let envelope = repository
            .load_intent_envelope(request_id)
            .await?
            .ok_or_else(|| DaemonError::FastPathPreparation {
                reason: format!("request {request_id} has no persisted intent envelope"),
            })?;
        let compiled =
            compile_intent(&envelope).map_err(|error| DaemonError::FastPathPreparation {
                reason: format!(
                    "request {request_id} could not be recompiled: {}",
                    format_compiler_failure(&error)
                ),
            })?;

        let route = FastPathRoute {
            request_id: request_id.to_owned(),
            intent_id: compiled.intent_id.clone(),
            venue: infer_fast_path_venue(&route_decision, &compiled),
            summary: route_decision.route.summary.clone(),
        };
        let template = template_from_compiled_intent(&compiled, &route).map_err(|error| {
            DaemonError::FastPathPreparation {
                reason: error.to_string(),
            }
        })?;

        prepare_fast_action(FastPathPreparationInput {
            route: &route,
            reservation_id,
            template,
            request_id,
        })
        .map_err(|error| DaemonError::FastPathPreparation {
            reason: error.to_string(),
        })
    }

    pub async fn execute_fast_path_action(
        &self,
        action: &PreparedFastAction,
        signer_peer: LocalPeerIdentity,
    ) -> Result<TxLifecycleReport, DaemonError> {
        self.reservations
            .require_held(&action.reservation_id, action_notional_usd(action))
            .await?;

        let approval_request = ApprovalRequest {
            action_id: action.action_id.clone(),
            action_kind: action.action_kind.clone(),
            reservation_id: action.reservation_id.clone(),
            notional_usd: action_notional_usd(action),
            origin_transport: self.control_plane.endpoint().transport(),
        };

        self.signer_bridge
            .request_approval_from_peer(&signer_peer, approval_request)
            .await?;

        let prepared_tx = prepared_action_into_evm_transaction(action);
        let sign_request = TxSignRequest {
            payload: prepared_tx.calldata.clone(),
        };
        let signed = self
            .signer_bridge
            .sign_transaction_from_peer(&signer_peer, sign_request)
            .await?;

        self.signer.handoff(&ExecutionRequest {
            action_id: action.action_id.clone(),
            action_kind: action.action_kind.clone(),
            notional_usd: action_notional_usd(action),
            reservation_id: action.reservation_id.clone(),
        });

        let repository = StateRepository::open(&self.state_db_path).await?;
        repository
            .persist_execution_state_with_metadata(
                &action.action_id,
                &action.action_kind,
                "prepared",
                Some(serde_json::json!({
                    "request_id": action.request_id,
                    "venue": action.venue,
                    "reservation_id": action.reservation_id,
                })),
            )
            .await?;

        let report = self
            .evm_adapter
            .submit_and_watch(
                prepared_tx.clone(),
                SignedTransactionBytes {
                    bytes: signed.bytes,
                },
            )
            .await?;

        for event in &report.events {
            repository
                .persist_execution_state_with_metadata(
                    &action.action_id,
                    &action.action_kind,
                    tx_status_label(&event.status),
                    Some(serde_json::to_value(&event.metadata).map_err(|source| {
                        DaemonError::FastPathPreparation {
                            reason: source.to_string(),
                        }
                    })?),
                )
                .await?;
        }

        if matches!(report.terminal_status(), Some(TxLifecycleStatus::Confirmed)) {
            self.reservations
                .consume(&action.reservation_id, action_notional_usd(action))
                .await?;
            self.reservations
                .release(&action.reservation_id, Some(action_notional_usd(action)))
                .await?;
        }

        if let Err(error) =
            project_execution_analytics(&self.state_db_path, &self.analytics_db_path).await
        {
            tracing::warn!(
                analytics_db = %self.analytics_db_path.display(),
                execution_id = %action.action_id,
                error = %error,
                "analytics projection failed after canonical commit"
            );
        }

        Ok(report)
    }

    pub async fn register_strategy(
        &self,
        request: JsonRpcRequest<serde_json::Value>,
    ) -> Result<JsonRpcResponse<StrategyRegistrationReceipt>, DaemonError> {
        let request_id = request.id.clone();
        let decoded = match decode_strategy_registration(self.control_plane.endpoint(), request) {
            Ok(decoded) => decoded,
            Err(DaemonError::InvalidStrategyEnvelope { source }) => {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_030,
                    format!("invalid strategy payload: {source}"),
                ));
            }
            Err(DaemonError::UnexpectedAgentRequestKind { kind }) => {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_031,
                    format!("strategy registration requires request_kind=strategy, got {kind}"),
                ));
            }
            Err(error) => return Err(error),
        };

        self.recorded_strategies
            .lock()
            .expect("recorded strategy store lock")
            .push(decoded.request.clone());

        let repository = StateRepository::open(&self.state_db_path).await?;
        repository
            .persist_strategy_registration(&decoded.request)
            .await?;

        let compiled = match compile_strategy(&decoded.request) {
            Ok(compiled) => compiled,
            Err(error) => {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_032,
                    format_compiler_failure(&error),
                ));
            }
        };
        let route_decision =
            classify_gateway_route(&CompiledAgentRequest::Strategy(compiled.clone()))
                .route_decision();
        repository
            .persist_route_decision(
                &decoded.request.request_id,
                "strategy",
                &decoded.request.payload.strategy_id,
                &route_decision,
                &decoded.request.submitted_at,
            )
            .await?;
        self.compiled_strategies
            .lock()
            .expect("compiled strategy store lock")
            .push(compiled);

        Ok(JsonRpcResponse::success(
            request_id,
            StrategyRegistrationReceipt {
                request_id: decoded.request.request_id,
                strategy_id: decoded.request.payload.strategy_id,
                request_kind: decoded.request.request_kind,
            },
        ))
    }

    pub async fn restore_active_strategies(
        &self,
        recovered_at: &str,
    ) -> Result<Vec<String>, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let snapshots = repository.load_recoverable_strategy_snapshots().await?;
        let engine = StrategyRuntimeEngine;
        let mut restored = Vec::new();

        for snapshot in snapshots {
            let strategy_id = snapshot.strategy.strategy_id.clone();
            let runtime = strategy_runtime_snapshot(snapshot.clone())?;
            let restored_snapshot = engine.restore(runtime, recovered_at);
            repository
                .persist_strategy_recovery_snapshot(&persisted_runtime_snapshot(
                    snapshot.strategy,
                    restored_snapshot,
                ))
                .await?;
            let persisted = repository
                .load_strategy_recovery_snapshot(&strategy_id)
                .await?
                .ok_or_else(|| DaemonError::FastPathPreparation {
                    reason: format!("strategy {strategy_id} recovery snapshot disappeared"),
                })?;
            self.spawn_runtime_supervisor(&strategy_id, strategy_runtime_snapshot(persisted)?);
            restored.push(strategy_id);
        }

        Ok(restored)
    }

    pub async fn inspect_runtime_control(&self) -> Result<PersistedRuntimeControl, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        Ok(repository
            .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
            .await?
            .unwrap_or_else(default_runtime_control_record))
    }

    pub async fn stop_runtime(
        &self,
        reason: &str,
        source: &str,
        transitioned_at: &str,
    ) -> Result<PersistedRuntimeControl, DaemonError> {
        self.set_runtime_control_mode(
            RUNTIME_CONTROL_MODE_STOPPED,
            reason,
            source,
            transitioned_at,
        )
        .await
    }

    pub async fn pause_runtime(
        &self,
        reason: &str,
        source: &str,
        transitioned_at: &str,
    ) -> Result<PersistedRuntimeControl, DaemonError> {
        self.set_runtime_control_mode(RUNTIME_CONTROL_MODE_PAUSED, reason, source, transitioned_at)
            .await
    }

    pub async fn clear_runtime_stop(
        &self,
        reason: &str,
        source: &str,
        cleared_at: &str,
    ) -> Result<PersistedRuntimeControl, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let mut record = repository
            .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
            .await?
            .unwrap_or_else(default_runtime_control_record);
        record.control_mode = RUNTIME_CONTROL_MODE_ACTIVE.to_owned();
        record.transition_reason = reason.to_owned();
        record.transition_source = source.to_owned();
        record.transitioned_at = cleared_at.to_owned();
        record.last_cleared_at = Some(cleared_at.to_owned());
        record.last_cleared_reason = Some(reason.to_owned());
        record.last_cleared_source = Some(source.to_owned());
        record.updated_at = cleared_at.to_owned();
        repository.persist_runtime_control(&record).await?;
        Ok(record)
    }

    async fn set_runtime_control_mode(
        &self,
        control_mode: &str,
        reason: &str,
        source: &str,
        transitioned_at: &str,
    ) -> Result<PersistedRuntimeControl, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let mut record = repository
            .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
            .await?
            .unwrap_or_else(default_runtime_control_record);
        record.control_mode = control_mode.to_owned();
        record.transition_reason = reason.to_owned();
        record.transition_source = source.to_owned();
        record.transitioned_at = transitioned_at.to_owned();
        record.updated_at = transitioned_at.to_owned();
        repository.persist_runtime_control(&record).await?;
        Ok(record)
    }

    pub async fn evaluate_strategy(
        &self,
        strategy_id: &str,
        samples: Vec<RuntimeWatcherState>,
        now: &str,
    ) -> Result<Vec<RuntimeCommand>, DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let snapshot = repository
            .load_strategy_recovery_snapshot(strategy_id)
            .await?
            .ok_or_else(|| DaemonError::FastPathPreparation {
                reason: format!("strategy {strategy_id} has no recovery snapshot"),
            })?;
        let engine = StrategyRuntimeEngine;
        let evaluation =
            engine.evaluate(strategy_runtime_snapshot(snapshot.clone())?, samples, now);
        repository
            .persist_strategy_recovery_snapshot(&persisted_runtime_snapshot(
                snapshot.strategy,
                evaluation.snapshot.clone(),
            ))
            .await?;
        Ok(evaluation.commands)
    }

    async fn enforce_runtime_control_gate(
        &self,
        strategy_id: &str,
        action_kind: &str,
        attempted_at: &str,
    ) -> Result<(), DaemonError> {
        let repository = StateRepository::open(&self.state_db_path).await?;
        let Some(mut control) = repository
            .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
            .await?
        else {
            return Ok(());
        };

        let Some((code, message)) =
            runtime_control_rejection_details(&control.control_mode, action_kind)
        else {
            return Ok(());
        };

        control.last_rejection_code = Some(code.to_owned());
        control.last_rejection_message = Some(message.clone());
        control.last_rejection_operation = Some(action_kind.to_owned());
        control.last_rejection_at = Some(attempted_at.to_owned());
        control.updated_at = attempted_at.to_owned();
        repository.persist_runtime_control(&control).await?;

        if control.control_mode == RUNTIME_CONTROL_MODE_STOPPED {
            let Some(snapshot) = repository
                .load_strategy_recovery_snapshot(strategy_id)
                .await?
            else {
                return Err(DaemonError::FastPathPreparation {
                    reason: format!("strategy {strategy_id} has no recovery snapshot"),
                });
            };
            let mut next_snapshot = strategy_runtime_snapshot(snapshot.clone())?;
            next_snapshot.runtime_state = MANUAL_STOP_RUNTIME_STATE.to_owned();
            set_snapshot_metric_bool(&mut next_snapshot.metrics, "warm", false);
            repository
                .persist_strategy_recovery_snapshot(&persisted_runtime_snapshot(
                    snapshot.strategy,
                    next_snapshot,
                ))
                .await?;
        }

        Err(DaemonError::RuntimeControlBlocked {
            code: code.to_owned(),
            message,
        })
    }

    pub async fn execute_stateful_hedge(
        &self,
        strategy_id: &str,
        command: RuntimeCommand,
        reservation_id: &str,
        signer_peer: LocalPeerIdentity,
        synced_at: &str,
    ) -> Result<TxLifecycleReport, DaemonError> {
        let hedge = match command {
            RuntimeCommand::Rebalance(hedge) | RuntimeCommand::Unwind(hedge) => hedge,
        };
        let action_kind = if hedge.reduce_only {
            "strategy_unwind"
        } else {
            "strategy_rebalance"
        };
        self.enforce_runtime_control_gate(strategy_id, action_kind, synced_at)
            .await?;
        enforce_policy(
            &self.policy,
            &prepared_hedge_action_id(&hedge),
            action_kind,
            hedge.notional_usd,
        )?;
        self.reservations
            .require_held(reservation_id, hedge.notional_usd)
            .await?;

        let repository = StateRepository::open(&self.state_db_path).await?;
        let snapshot = repository
            .load_strategy_recovery_snapshot(strategy_id)
            .await?
            .ok_or_else(|| DaemonError::FastPathPreparation {
                reason: format!("strategy {strategy_id} has no recovery snapshot"),
            })?;
        let (signer_address, account_address) = snapshot
            .pending_hedge
            .as_ref()
            .map(|pending| {
                (
                    pending.signer_address.clone(),
                    pending.account_address.clone(),
                )
            })
            .unwrap_or_else(|| stateful_hedge_identities(strategy_id));
        let prepared = self.hyperliquid_adapter.prepare_order(
            snapshot.pending_hedge.as_ref().map(|hedge| hedge.nonce),
            HedgeOrderRequest {
                strategy_id: hedge.strategy_id.clone(),
                instrument: hedge.instrument.clone(),
                notional_usd: hedge.notional_usd,
                reduce_only: hedge.reduce_only,
            },
        );

        self.signer_bridge
            .request_approval_from_peer(
                &signer_peer,
                ApprovalRequest {
                    action_id: prepared.client_order_id.clone(),
                    action_kind: action_kind.to_owned(),
                    reservation_id: reservation_id.to_owned(),
                    notional_usd: hedge.notional_usd,
                    origin_transport: self.control_plane.endpoint().transport(),
                },
            )
            .await?;
        self.signer.handoff(&ExecutionRequest {
            action_id: prepared.client_order_id.clone(),
            action_kind: action_kind.to_owned(),
            notional_usd: hedge.notional_usd,
            reservation_id: reservation_id.to_owned(),
        });

        repository
            .persist_execution_state_with_metadata(
                &prepared.client_order_id,
                action_kind,
                "submitted",
                Some(serde_json::json!({
                    "strategy_id": strategy_id,
                    "reservation_id": reservation_id,
                    "instrument": hedge.instrument,
                    "venue": "hyperliquid",
                    "signer_address": signer_address,
                    "account_address": account_address,
                })),
            )
            .await?;

        let ack = self
            .hyperliquid_adapter
            .place_hedge_order(HyperliquidHedgeSubmitRequest {
                prepared: prepared.clone(),
                signer_address: signer_address.clone(),
                account_address: account_address.clone(),
                asset: 0,
                is_buy: !hedge.reduce_only,
                price: hedge.notional_usd.to_string(),
                size: "1.0".to_owned(),
                time_in_force: "Ioc".to_owned(),
            })
            .await?;
        let sync = self
            .hyperliquid_adapter
            .sync_state(HyperliquidSyncRequest {
                signer_address: ack.signer_address.clone(),
                account_address: ack.account_address.clone(),
                order_id: ack.order_id,
                aggregate_fills: true,
            })
            .await?;
        let synced_status = synced_status(&prepared.client_order_id, &ack.status, &sync);
        let report = adapter_native_report(&prepared, ack.order_id, &synced_status);
        let mut next_snapshot = strategy_runtime_snapshot(snapshot.clone())?;
        next_snapshot.runtime_state = "active".to_owned();
        set_snapshot_metric_bool(&mut next_snapshot.metrics, "warm", false);
        set_snapshot_metric_bool(&mut next_snapshot.metrics, "venue_sync_required", false);
        next_snapshot.pending_hedge = Some(RuntimePendingHedge {
            venue: prepared.venue,
            instrument: prepared.instrument,
            client_order_id: prepared.client_order_id.clone(),
            signer_address: ack.signer_address.clone(),
            account_address: ack.account_address.clone(),
            order_id: ack.order_id,
            nonce: ack.nonce,
            status: synced_status.clone(),
            last_synced_at: Some(synced_at.to_owned()),
        });
        repository
            .persist_strategy_recovery_snapshot(&persisted_runtime_snapshot(
                snapshot.strategy,
                next_snapshot,
            ))
            .await?;
        repository
            .persist_execution_state_with_metadata(
                &prepared.client_order_id,
                action_kind,
                &synced_status,
                Some(serde_json::json!({
                    "client_order_id": prepared.client_order_id,
                    "order_id": ack.order_id,
                    "nonce": ack.nonce,
                    "queried_account": sync.queried_account,
                    "queried_signer": sync.queried_signer,
                    "open_orders": sync.open_orders,
                    "order_status": sync.order_status,
                    "fills": sync.fills,
                    "positions": sync.positions,
                })),
            )
            .await?;

        if matches!(report.terminal_status(), Some(TxLifecycleStatus::Confirmed)) {
            self.reservations
                .consume(reservation_id, hedge.notional_usd)
                .await?;
            self.reservations
                .release(reservation_id, Some(hedge.notional_usd))
                .await?;
        }

        Ok(report)
    }

    pub async fn sync_strategy_hedge(
        &self,
        strategy_id: &str,
        synced_at: &str,
    ) -> Result<(), DaemonError> {
        sync_strategy_hedge_with_adapter(
            &self.state_db_path,
            &self.hyperliquid_adapter,
            strategy_id,
            synced_at,
        )
        .await
    }

    pub fn recorded_strategies(&self) -> Vec<AgentRequestEnvelope<Strategy>> {
        self.recorded_strategies
            .lock()
            .expect("recorded strategy store lock")
            .clone()
    }

    pub fn compiled_strategies(&self) -> Vec<CompiledStrategy> {
        self.compiled_strategies
            .lock()
            .expect("compiled strategy store lock")
            .clone()
    }

    pub fn authorize_request(
        &self,
        request: JsonRpcRequest<ExecutionRequest>,
    ) -> Result<JsonRpcResponse<AuthorizationResult>, DaemonError> {
        let request_id = request.id.clone();
        let decoded = decode_control_request(self.control_plane.endpoint(), request)?;
        let policy_input = PolicyInput {
            action_id: decoded.request.action_id.clone(),
            action_kind: decoded.request.action_kind.clone(),
            notional_usd: decoded.request.notional_usd,
        };

        let verdict = match self.policy.evaluate(&policy_input) {
            PolicyDecision::Allow => AuthorizationVerdict::Allow,
            PolicyDecision::AllowWithModifications { modifications } => {
                AuthorizationVerdict::AllowWithModifications { modifications }
            }
            PolicyDecision::Hold { reason } => AuthorizationVerdict::Hold { reason },
            PolicyDecision::Reject { reason } => {
                return Ok(JsonRpcResponse::failure(request_id, -32_010, reason));
            }
        };

        let _ = &self.signer;

        Ok(JsonRpcResponse::success(
            request_id,
            AuthorizationResult {
                action_id: decoded.request.action_id,
                verdict,
            },
        ))
    }

    pub async fn authorize_and_prepare_execution(
        &self,
        request: JsonRpcRequest<ExecutionRequest>,
    ) -> Result<JsonRpcResponse<AuthorizationResult>, DaemonError> {
        let request_id = request.id.clone();
        let decoded = decode_control_request(self.control_plane.endpoint(), request)?;
        let policy_input = PolicyInput {
            action_id: decoded.request.action_id.clone(),
            action_kind: decoded.request.action_kind.clone(),
            notional_usd: decoded.request.notional_usd,
        };

        let verdict = match self.policy.evaluate(&policy_input) {
            PolicyDecision::Allow => AuthorizationVerdict::Allow,
            PolicyDecision::AllowWithModifications { modifications } => {
                AuthorizationVerdict::AllowWithModifications { modifications }
            }
            PolicyDecision::Hold { reason } => AuthorizationVerdict::Hold { reason },
            PolicyDecision::Reject { reason } => {
                return Ok(JsonRpcResponse::failure(request_id, -32_010, reason));
            }
        };

        if matches!(
            verdict,
            AuthorizationVerdict::Allow | AuthorizationVerdict::AllowWithModifications { .. }
        ) {
            if let Err(error) = self
                .reservations
                .consume(
                    &decoded.request.reservation_id,
                    decoded.request.notional_usd,
                )
                .await
            {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_011,
                    format!(
                        "execution requires an active reservation hold for {}: {}",
                        decoded.request.reservation_id, error
                    ),
                ));
            }
            if let Err(error) = self
                .reservations
                .release(
                    &decoded.request.reservation_id,
                    Some(decoded.request.notional_usd),
                )
                .await
            {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_012,
                    format!(
                        "execution cannot release reservation {}: {}",
                        decoded.request.reservation_id, error
                    ),
                ));
            }
        }

        let _ = &self.signer;

        Ok(JsonRpcResponse::success(
            request_id,
            AuthorizationResult {
                action_id: decoded.request.action_id,
                verdict,
            },
        ))
    }

    pub async fn authorize_and_dispatch_to_signer(
        &self,
        request: JsonRpcRequest<ExecutionRequest>,
        signer_peer: LocalPeerIdentity,
    ) -> Result<JsonRpcResponse<AuthorizationResult>, DaemonError> {
        let request_id = request.id.clone();
        let decoded = decode_control_request(self.control_plane.endpoint(), request)?;
        let policy_input = PolicyInput {
            action_id: decoded.request.action_id.clone(),
            action_kind: decoded.request.action_kind.clone(),
            notional_usd: decoded.request.notional_usd,
        };

        let verdict = match self.policy.evaluate(&policy_input) {
            PolicyDecision::Allow => AuthorizationVerdict::Allow,
            PolicyDecision::AllowWithModifications { modifications } => {
                AuthorizationVerdict::AllowWithModifications { modifications }
            }
            PolicyDecision::Hold { reason } => AuthorizationVerdict::Hold { reason },
            PolicyDecision::Reject { reason } => {
                return Ok(JsonRpcResponse::failure(request_id, -32_010, reason));
            }
        };

        if matches!(
            verdict,
            AuthorizationVerdict::Allow | AuthorizationVerdict::AllowWithModifications { .. }
        ) {
            if let Err(error) = self
                .reservations
                .require_held(
                    &decoded.request.reservation_id,
                    decoded.request.notional_usd,
                )
                .await
            {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_011,
                    format!(
                        "execution requires an active reservation hold for {}: {}",
                        decoded.request.reservation_id, error
                    ),
                ));
            }

            let approval_request = ApprovalRequest {
                action_id: decoded.request.action_id.clone(),
                action_kind: decoded.request.action_kind.clone(),
                reservation_id: decoded.request.reservation_id.clone(),
                notional_usd: decoded.request.notional_usd,
                origin_transport: decoded.transport,
            };

            if let Err(error) = self
                .signer_bridge
                .request_approval_from_peer(&signer_peer, approval_request)
                .await
            {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_013,
                    error.to_string(),
                ));
            }

            self.signer.handoff(&decoded.request);

            if let Err(error) = self
                .reservations
                .consume(
                    &decoded.request.reservation_id,
                    decoded.request.notional_usd,
                )
                .await
            {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_011,
                    format!(
                        "execution requires an active reservation hold for {}: {}",
                        decoded.request.reservation_id, error
                    ),
                ));
            }

            if let Err(error) = self
                .reservations
                .release(
                    &decoded.request.reservation_id,
                    Some(decoded.request.notional_usd),
                )
                .await
            {
                return Ok(JsonRpcResponse::failure(
                    request_id,
                    -32_012,
                    format!(
                        "execution cannot release reservation {}: {}",
                        decoded.request.reservation_id, error
                    ),
                ));
            }

            let repository = StateRepository::open(&self.state_db_path).await?;
            repository
                .persist_execution_state(
                    &decoded.request.action_id,
                    &decoded.request.action_kind,
                    "dispatched",
                )
                .await?;

            if let Err(error) =
                project_execution_analytics(&self.state_db_path, &self.analytics_db_path).await
            {
                tracing::warn!(
                    analytics_db = %self.analytics_db_path.display(),
                    execution_id = %decoded.request.action_id,
                    error = %error,
                    "analytics projection failed after canonical commit"
                );
            }
        }

        Ok(JsonRpcResponse::success(
            request_id,
            AuthorizationResult {
                action_id: decoded.request.action_id,
                verdict,
            },
        ))
    }

    pub async fn serve_once<T>(&self, io: T) -> Result<(), DaemonError>
    where
        T: AsyncRead + AsyncWrite + Unpin,
    {
        let mut framed = frame_transport(io);
        let request = recv_json_message(&mut framed)
            .await
            .map_err(DaemonError::Ipc)?;
        let response = self.authorize_and_prepare_execution(request).await?;
        send_json_message(&mut framed, &response)
            .await
            .map_err(DaemonError::Ipc)
    }
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("failed to resolve current directory")]
    CurrentDir(#[source] std::io::Error),
    #[error("failed to create daemon data directory at {path}")]
    CreateDataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open sqlite database at {path}")]
    OpenSqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to initialize sqlite database at {path}")]
    InitializeSqlite {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("state repository error")]
    State(#[from] StateError),
    #[error("failed to inspect daemon state at {path}")]
    ProbeState {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to wait for shutdown signal")]
    ShutdownSignal(#[source] std::io::Error),
    #[error("daemon task join failed")]
    Join(#[source] tokio::task::JoinError),
    #[error("unsupported daemon control method: {method}")]
    UnsupportedControlMethod { method: String },
    #[error("failed to exchange framed IPC messages")]
    Ipc(#[source] IpcError),
    #[error("failed to decode intent submission envelope")]
    InvalidIntentEnvelope {
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to decode strategy registration envelope")]
    InvalidStrategyEnvelope {
        #[source]
        source: serde_json::Error,
    },
    #[error("unexpected agent request kind: {kind}")]
    UnexpectedAgentRequestKind { kind: String },
    #[error("signer bridge error")]
    SignerBridge(#[from] SignerBridgeError),
    #[error(transparent)]
    EvmAdapter(#[from] EvmAdapterError),
    #[error(transparent)]
    AcrossAdapter(#[from] AcrossAdapterError),
    #[error(transparent)]
    PredictionMarketAdapter(#[from] PredictionMarketAdapterError),
    #[error(transparent)]
    HyperliquidAdapter(#[from] HyperliquidAdapterError),
    #[error(transparent)]
    Planner(#[from] a2ex_planner::PlannerError),
    #[error("fast-path preparation failed: {reason}")]
    FastPathPreparation { reason: String },
    #[error("planned execution failed: {reason}")]
    PlanExecution { reason: String },
    #[error("{code}: {message}")]
    RuntimeControlBlocked { code: String, message: String },
    #[error("reservation error")]
    Reservation(#[from] ReservationError),
    #[error("authorization failed: {reason}")]
    Authorization { reason: String },
}

pub fn init_tracing() {
    let _ =
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::from_default_env().add_directive(Level::INFO.into())
            }))
            .with_target(false)
            .try_init();
}

pub struct DaemonServiceHandle<P, R, S, B = NoopSignerBridge, A = NoopEvmAdapter> {
    bootstrap: BootstrapReport,
    service: DaemonService<P, R, S, B, A>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), DaemonError>>,
}

pub type DaemonHandle =
    DaemonServiceHandle<BaselinePolicy, SqliteReservationManager, NoopRuntimeSigner>;

impl<P, R, S, B, A> DaemonServiceHandle<P, R, S, B, A>
where
    P: PolicyEvaluator,
    R: ReservationManager,
    S: SignerHandoff,
    B: ValidatedSignerBridge,
    A: EvmAdapter,
{
    pub fn bootstrap(&self) -> &BootstrapReport {
        &self.bootstrap
    }

    pub fn active_runtime_supervisors(&self) -> Vec<String> {
        self.service.active_runtime_supervisors()
    }

    pub fn take_runtime_commands(&self, strategy_id: &str) -> Vec<RuntimeCommand> {
        self.service.take_runtime_commands(strategy_id)
    }

    pub async fn publish_runtime_watcher_sample(
        &self,
        strategy_id: &str,
        sample: RuntimeWatcherState,
    ) -> Result<(), DaemonError> {
        self.service
            .publish_runtime_watcher_sample(strategy_id, sample)
            .await
    }

    pub async fn shutdown(mut self) -> Result<BootstrapReport, DaemonError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        self.task.await.map_err(DaemonError::Join)??;
        Ok(self.bootstrap)
    }
}

pub async fn bootstrap_local_runtime(
    config: &DaemonConfig,
) -> Result<BootstrapReport, DaemonError> {
    fs::create_dir_all(config.data_dir())
        .await
        .map_err(|source| DaemonError::CreateDataDir {
            path: config.data_dir().to_path_buf(),
            source,
        })?;

    let recovered_existing_state =
        fs::try_exists(config.state_db_path())
            .await
            .map_err(|source| DaemonError::ProbeState {
                path: config.state_db_path(),
                source,
            })?;

    let state_repository = StateRepository::open(config.state_db_path()).await?;
    let _ = state_repository.load_snapshot().await?;
    let engine = StrategyRuntimeEngine;
    for snapshot in state_repository
        .load_recoverable_strategy_snapshots()
        .await?
    {
        let restored = engine.restore(
            strategy_runtime_snapshot(snapshot.clone())?,
            BOOTSTRAP_RECOVERED_AT,
        );
        state_repository
            .persist_strategy_recovery_snapshot(&persisted_runtime_snapshot(
                snapshot.strategy,
                restored,
            ))
            .await?;
    }
    initialize_sqlite(&config.analytics_db_path(), "analytics").await?;

    Ok(BootstrapReport {
        source: BootstrapSource::LocalRuntime,
        bootstrap_path: BOOTSTRAP_PATH.to_owned(),
        state_db_path: config.state_db_path(),
        analytics_db_path: config.analytics_db_path(),
        used_remote_control_plane: false,
        recovered_existing_state,
    })
}

pub async fn spawn_local_daemon_with_service<P, R, S, B, A>(
    config: DaemonConfig,
    service: DaemonService<P, R, S, B, A>,
) -> Result<DaemonServiceHandle<P, R, S, B, A>, DaemonError>
where
    P: PolicyEvaluator + Send + Sync + 'static,
    R: ReservationManager + 'static,
    S: SignerHandoff + 'static,
    B: ValidatedSignerBridge + 'static,
    A: EvmAdapter + 'static,
{
    let bootstrap = bootstrap_local_runtime(&config).await?;
    let _ = service
        .restore_active_strategies(BOOTSTRAP_RECOVERED_AT)
        .await?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = shutdown_rx.await;
        Ok(())
    });

    Ok(DaemonServiceHandle {
        bootstrap,
        service,
        shutdown_tx: Some(shutdown_tx),
        task,
    })
}

pub async fn spawn_local_daemon(config: DaemonConfig) -> Result<DaemonHandle, DaemonError> {
    let reservations = SqliteReservationManager::open(config.state_db_path()).await?;
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::default(),
        reservations,
        Arc::new(NoopRuntimeSigner),
    );
    spawn_local_daemon_with_service(config, service).await
}

pub async fn run_until_shutdown_signal(
    config: DaemonConfig,
) -> Result<BootstrapReport, DaemonError> {
    let daemon: DaemonHandle = spawn_local_daemon(config).await?;
    let bootstrap = daemon.bootstrap().clone();
    tracing::info!(
        state_db = %bootstrap.state_db_path.display(),
        analytics_db = %bootstrap.analytics_db_path.display(),
        bootstrap_path = %bootstrap.bootstrap_path,
        "a2ex daemon booted from local runtime"
    );

    wait_for_shutdown_signal().await?;
    daemon.shutdown().await?;
    tracing::info!("a2ex daemon shutdown complete");

    Ok(bootstrap)
}

pub async fn run_until(
    config: DaemonConfig,
    shutdown: impl Future<Output = ()>,
) -> Result<BootstrapReport, DaemonError> {
    let daemon: DaemonHandle = spawn_local_daemon(config).await?;
    let bootstrap = daemon.bootstrap().clone();
    shutdown.await;
    daemon.shutdown().await?;
    Ok(bootstrap)
}

pub async fn persist_canonical_state(
    config: &DaemonConfig,
    snapshot: &CanonicalStateSnapshot,
) -> Result<(), DaemonError> {
    let repository = StateRepository::open(config.state_db_path()).await?;
    repository.persist_snapshot(snapshot).await?;
    Ok(())
}

pub async fn load_runtime_state(
    config: &DaemonConfig,
) -> Result<CanonicalStateSnapshot, DaemonError> {
    let repository = StateRepository::open(config.state_db_path()).await?;
    Ok(repository.load_snapshot().await?)
}

pub async fn load_event_journal(config: &DaemonConfig) -> Result<Vec<JournalEntry>, DaemonError> {
    let repository = StateRepository::open(config.state_db_path()).await?;
    Ok(repository.load_journal().await?)
}

pub fn decode_control_request(
    endpoint: &LocalControlEndpoint,
    request: JsonRpcRequest<ExecutionRequest>,
) -> Result<DecodedControlRequest, DaemonError> {
    if request.method != DAEMON_CONTROL_METHOD {
        return Err(DaemonError::UnsupportedControlMethod {
            method: request.method,
        });
    }

    Ok(DecodedControlRequest {
        transport: endpoint.transport(),
        request: request.params,
    })
}

pub fn decode_intent_submission(
    endpoint: &LocalControlEndpoint,
    request: JsonRpcRequest<serde_json::Value>,
) -> Result<DecodedIntentSubmission, DaemonError> {
    if request.method != DAEMON_SUBMIT_INTENT_METHOD {
        return Err(DaemonError::UnsupportedControlMethod {
            method: request.method,
        });
    }

    let envelope: AgentRequestEnvelope<Intent> = serde_json::from_value(request.params)
        .map_err(|source| DaemonError::InvalidIntentEnvelope { source })?;

    if envelope.request_kind != AgentRequestKind::Intent {
        return Err(DaemonError::UnexpectedAgentRequestKind {
            kind: format!("{:?}", envelope.request_kind).to_lowercase(),
        });
    }

    Ok(DecodedIntentSubmission {
        transport: endpoint.transport(),
        request: envelope,
    })
}

pub fn decode_strategy_registration(
    endpoint: &LocalControlEndpoint,
    request: JsonRpcRequest<serde_json::Value>,
) -> Result<DecodedStrategyRegistration, DaemonError> {
    if request.method != DAEMON_REGISTER_STRATEGY_METHOD {
        return Err(DaemonError::UnsupportedControlMethod {
            method: request.method,
        });
    }

    let envelope: AgentRequestEnvelope<Strategy> = serde_json::from_value(request.params)
        .map_err(|source| DaemonError::InvalidStrategyEnvelope { source })?;

    if envelope.request_kind != AgentRequestKind::Strategy {
        return Err(DaemonError::UnexpectedAgentRequestKind {
            kind: format!("{:?}", envelope.request_kind).to_lowercase(),
        });
    }

    Ok(DecodedStrategyRegistration {
        transport: endpoint.transport(),
        request: envelope,
    })
}

async fn wait_for_shutdown_signal() -> Result<(), DaemonError> {
    tokio::signal::ctrl_c()
        .await
        .map_err(DaemonError::ShutdownSignal)
}

async fn initialize_sqlite(path: &Path, store_kind: &'static str) -> Result<(), DaemonError> {
    let path_buf = path.to_path_buf();
    let connection = Connection::open(path)
        .await
        .map_err(|source| DaemonError::OpenSqlite {
            path: path_buf.clone(),
            source,
        })?;

    connection
        .call(move |conn| {
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS daemon_bootstrap_events (
                     event_id INTEGER PRIMARY KEY,
                     store_kind TEXT NOT NULL,
                     bootstrap_path TEXT NOT NULL,
                     occurred_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );",
            )?;
            conn.execute(
                "INSERT INTO daemon_bootstrap_events (store_kind, bootstrap_path) VALUES (?1, ?2)",
                (store_kind, BOOTSTRAP_PATH),
            )?;
            Ok(())
        })
        .await
        .map_err(|source| DaemonError::InitializeSqlite {
            path: path_buf,
            source,
        })
}

fn infer_fast_path_venue(
    route_decision: &a2ex_state::PersistedRouteDecision,
    compiled: &CompiledIntent,
) -> String {
    if let GatewayVerdict::FastPath(route) =
        classify_gateway_route(&CompiledAgentRequest::Intent(compiled.clone()))
    {
        return route.venue;
    }

    if let Some(first) = compiled.constraints.allowed_venues.first() {
        return first.clone();
    }

    route_decision.source_id.clone()
}

fn action_notional_usd(action: &PreparedFastAction) -> u64 {
    match &action.payload {
        PreparedVenueAction::GenericContractCall {
            reservation_amount_usd,
            ..
        } => *reservation_amount_usd,
        PreparedVenueAction::SimpleEntry { notional_usd, .. } => *notional_usd,
        PreparedVenueAction::HedgeAdjustPrecomputed { notional_usd, .. } => *notional_usd,
    }
}

fn prepared_hedge_action_id(hedge: &a2ex_strategy_runtime::HedgeCommand) -> String {
    format!("{}-{}", hedge.strategy_id, hedge.reason)
}

fn enforce_policy<P: PolicyEvaluator>(
    policy: &P,
    action_id: &str,
    action_kind: &str,
    notional_usd: u64,
) -> Result<(), DaemonError> {
    match policy.evaluate(&PolicyInput {
        action_id: action_id.to_owned(),
        action_kind: action_kind.to_owned(),
        notional_usd,
    }) {
        PolicyDecision::Allow | PolicyDecision::AllowWithModifications { .. } => Ok(()),
        PolicyDecision::Hold { reason } | PolicyDecision::Reject { reason } => {
            Err(DaemonError::Authorization { reason })
        }
    }
}

fn synced_status(
    client_order_id: &str,
    fallback_status: &str,
    sync: &a2ex_hyperliquid_adapter::HyperliquidSyncSnapshot,
) -> String {
    if let Some(order_status) = &sync.order_status {
        return order_status.status.clone();
    }
    if let Some(open_order) = sync
        .open_orders
        .iter()
        .find(|order| order.client_order_id.as_deref() == Some(client_order_id))
    {
        return open_order.status.clone();
    }
    if !sync.fills.is_empty() || !sync.positions.is_empty() {
        return "filled".to_owned();
    }
    fallback_status.to_owned()
}

fn set_snapshot_metric_bool(metrics: &mut serde_json::Value, key: &str, value: bool) {
    if let Some(object) = metrics.as_object_mut() {
        object.insert(key.to_owned(), serde_json::Value::Bool(value));
    } else {
        *metrics = serde_json::json!({ key: value });
    }
}

fn snapshot_metric_bool(metrics: &serde_json::Value, key: &str) -> bool {
    metrics
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn runtime_event_timestamp(event: &RuntimeEvent) -> String {
    match event {
        RuntimeEvent::WatcherSample(sample) => sample.sampled_at.clone(),
        RuntimeEvent::Tick { now } => now.clone(),
    }
}

fn current_runtime_timestamp() -> Option<String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(format_rfc3339_utc(epoch))
}

async fn sync_strategy_hedge_with_adapter(
    state_db_path: &Path,
    hyperliquid_adapter: &HyperliquidAdapter,
    strategy_id: &str,
    synced_at: &str,
) -> Result<(), DaemonError> {
    let repository = StateRepository::open(state_db_path).await?;
    let snapshot = repository
        .load_strategy_recovery_snapshot(strategy_id)
        .await?
        .ok_or_else(|| DaemonError::FastPathPreparation {
            reason: format!("strategy {strategy_id} has no recovery snapshot"),
        })?;
    let mut next_snapshot = strategy_runtime_snapshot(snapshot.clone())?;
    next_snapshot.runtime_state = "active".to_owned();
    set_snapshot_metric_bool(&mut next_snapshot.metrics, "warm", false);
    set_snapshot_metric_bool(&mut next_snapshot.metrics, "venue_sync_required", false);

    if let Some(pending_hedge) = snapshot.pending_hedge.clone() {
        let sync = hyperliquid_adapter
            .sync_state(HyperliquidSyncRequest {
                signer_address: pending_hedge.signer_address.clone(),
                account_address: pending_hedge.account_address.clone(),
                order_id: pending_hedge.order_id,
                aggregate_fills: true,
            })
            .await?;
        next_snapshot.pending_hedge = Some(RuntimePendingHedge {
            venue: pending_hedge.venue,
            instrument: pending_hedge.instrument,
            client_order_id: pending_hedge.client_order_id.clone(),
            signer_address: pending_hedge.signer_address,
            account_address: pending_hedge.account_address,
            order_id: pending_hedge.order_id,
            nonce: pending_hedge.nonce,
            status: synced_status(&pending_hedge.client_order_id, &pending_hedge.status, &sync),
            last_synced_at: Some(synced_at.to_owned()),
        });
    }

    repository
        .persist_strategy_recovery_snapshot(&persisted_runtime_snapshot(
            snapshot.strategy,
            next_snapshot,
        ))
        .await?;
    Ok(())
}

async fn continue_runtime_after_sync(
    state_db_path: &Path,
    strategy_id: &str,
    watcher_states: Vec<RuntimeWatcherState>,
    now: &str,
    runtime_commands: &Arc<Mutex<BTreeMap<String, Vec<RuntimeCommand>>>>,
) -> Result<(), DaemonError> {
    let repository = StateRepository::open(state_db_path).await?;
    let snapshot = repository
        .load_strategy_recovery_snapshot(strategy_id)
        .await?
        .ok_or_else(|| DaemonError::FastPathPreparation {
            reason: format!("strategy {strategy_id} has no recovery snapshot"),
        })?;
    let engine = StrategyRuntimeEngine;
    let evaluation = engine.evaluate(
        strategy_runtime_snapshot(snapshot.clone())?,
        watcher_states,
        now,
    );
    repository
        .persist_strategy_recovery_snapshot(&persisted_runtime_snapshot(
            snapshot.strategy,
            evaluation.snapshot,
        ))
        .await?;
    if !evaluation.commands.is_empty() {
        runtime_commands
            .lock()
            .expect("runtime command lock")
            .entry(strategy_id.to_owned())
            .or_default()
            .extend(evaluation.commands);
    }
    Ok(())
}

pub fn project_strategy_runtime_monitoring(
    recovery: Option<&PersistedStrategyRecoverySnapshot>,
) -> StrategyRuntimeMonitoringProjection {
    let Some(recovery) = recovery else {
        return StrategyRuntimeMonitoringProjection {
            current_phase: "awaiting_runtime_identity".to_owned(),
            last_action: None,
            next_intended_action: None,
            last_runtime_failure: None,
        };
    };

    let last_action =
        recovery
            .pending_hedge
            .as_ref()
            .map(|hedge| StrategyRuntimeActionProjection {
                kind: "hedge_order".to_owned(),
                status: hedge.status.clone(),
                summary: format!(
                    "Last known hedge order on {} {} is {}.",
                    hedge.venue, hedge.instrument, hedge.status
                ),
                observed_at: hedge.last_synced_at.clone(),
            });

    let next_intended_action = if let Some(hedge) = recovery.pending_hedge.as_ref() {
        match hedge.status.as_str() {
            "submitted" | "resting" => Some(StrategyRuntimeActionProjection {
                kind: "sync_pending_hedge".to_owned(),
                status: "pending".to_owned(),
                summary: format!(
                    "Sync the pending hedge order on {} {} before assuming runtime progress.",
                    hedge.venue, hedge.instrument
                ),
                observed_at: hedge.last_synced_at.clone(),
            }),
            _ => None,
        }
    } else {
        recovery
            .next_tick_at
            .as_ref()
            .map(|next_tick_at| StrategyRuntimeActionProjection {
                kind: "await_next_tick".to_owned(),
                status: "scheduled".to_owned(),
                summary: format!(
                    "Await the next runtime supervisor tick scheduled at {}.",
                    next_tick_at
                ),
                observed_at: Some(next_tick_at.clone()),
            })
    };

    let last_runtime_failure =
        recovery
            .pending_hedge
            .as_ref()
            .and_then(|hedge| match hedge.status.as_str() {
                "failed" | "cancelled" | "rejected" => Some(StrategyRuntimeOutcomeProjection {
                    code: format!("hedge_{}", hedge.status),
                    message: format!(
                        "Last known hedge order on {} {} ended with status {}.",
                        hedge.venue, hedge.instrument, hedge.status
                    ),
                    observed_at: hedge
                        .last_synced_at
                        .clone()
                        .unwrap_or_else(|| recovery.updated_at.clone()),
                }),
                _ => None,
            });

    StrategyRuntimeMonitoringProjection {
        current_phase: recovery.runtime_state.clone(),
        last_action,
        next_intended_action,
        last_runtime_failure,
    }
}

pub fn project_strategy_runtime_reconciliation(
    execution: Option<&ExecutionStateRecord>,
    reconciliation: Option<&ReconciliationStateRecord>,
) -> StrategyRuntimeReconciliationProjection {
    match (execution, reconciliation) {
        (_, Some(reconciliation)) if reconciliation.rebalance_required => {
            StrategyRuntimeReconciliationProjection {
                status: "rebalance_required".to_owned(),
                summary: format!(
                    "Canonical reconciliation for execution {} still shows residual exposure {} USD and requires a follow-up rebalance.",
                    reconciliation.execution_id, reconciliation.residual_exposure_usd
                ),
                execution_id: Some(reconciliation.execution_id.clone()),
                execution_status: execution.map(|execution| execution.status.clone()),
                residual_exposure_usd: Some(reconciliation.residual_exposure_usd),
                rebalance_required: true,
                owner_action_needed: true,
                observed_at: Some(reconciliation.updated_at.clone()),
            }
        }
        (_, Some(reconciliation)) => StrategyRuntimeReconciliationProjection {
            status: "reconciled".to_owned(),
            summary: format!(
                "Canonical reconciliation for execution {} reports residual exposure {} USD with no follow-up rebalance required.",
                reconciliation.execution_id, reconciliation.residual_exposure_usd
            ),
            execution_id: Some(reconciliation.execution_id.clone()),
            execution_status: execution.map(|execution| execution.status.clone()),
            residual_exposure_usd: Some(reconciliation.residual_exposure_usd),
            rebalance_required: false,
            owner_action_needed: false,
            observed_at: Some(reconciliation.updated_at.clone()),
        },
        (Some(execution), None) => StrategyRuntimeReconciliationProjection {
            status: "pending".to_owned(),
            summary: format!(
                "Execution {} is recorded with status {} but no canonical reconciliation snapshot has been persisted yet.",
                execution.execution_id, execution.status
            ),
            execution_id: Some(execution.execution_id.clone()),
            execution_status: Some(execution.status.clone()),
            residual_exposure_usd: None,
            rebalance_required: false,
            owner_action_needed: false,
            observed_at: Some(execution.updated_at.clone()),
        },
        (None, None) => StrategyRuntimeReconciliationProjection {
            status: "not_started".to_owned(),
            summary: "No canonical autonomous execution or reconciliation evidence has been recorded for this strategy yet.".to_owned(),
            execution_id: None,
            execution_status: None,
            residual_exposure_usd: None,
            rebalance_required: false,
            owner_action_needed: false,
            observed_at: None,
        },
    }
}

fn runtime_control_rejection_details(
    control_mode: &str,
    action_kind: &str,
) -> Option<(&'static str, String)> {
    match control_mode {
        RUNTIME_CONTROL_MODE_PAUSED => Some((
            RUNTIME_REJECTION_CODE_PAUSED,
            format!(
                "runtime is paused; clear_stop before executing new autonomous actions ({action_kind})"
            ),
        )),
        RUNTIME_CONTROL_MODE_STOPPED => Some((
            RUNTIME_REJECTION_CODE_STOPPED,
            format!(
                "runtime is stopped; clear_stop before executing new strategy actions ({MANUAL_STOP_METRIC} aligned)"
            ),
        )),
        _ => None,
    }
}

fn default_runtime_control_record() -> PersistedRuntimeControl {
    PersistedRuntimeControl {
        scope_key: AUTONOMOUS_RUNTIME_CONTROL_SCOPE.to_owned(),
        control_mode: RUNTIME_CONTROL_MODE_ACTIVE.to_owned(),
        transition_reason: "initial_state".to_owned(),
        transition_source: "daemon".to_owned(),
        transitioned_at: BOOTSTRAP_RECOVERED_AT.to_owned(),
        last_cleared_at: None,
        last_cleared_reason: None,
        last_cleared_source: None,
        last_rejection_code: None,
        last_rejection_message: None,
        last_rejection_operation: None,
        last_rejection_at: None,
        updated_at: BOOTSTRAP_RECOVERED_AT.to_owned(),
    }
}

fn strategy_runtime_snapshot(
    snapshot: PersistedStrategyRecoverySnapshot,
) -> Result<StrategyRuntimeSnapshot, DaemonError> {
    Ok(StrategyRuntimeSnapshot {
        strategy: compile_strategy(&AgentRequestEnvelope {
            request_id: snapshot.strategy.request_id.clone(),
            request_kind: AgentRequestKind::Strategy,
            source_agent_id: snapshot.strategy.source_agent_id.clone(),
            submitted_at: snapshot.strategy.submitted_at.clone(),
            payload: Strategy {
                strategy_id: snapshot.strategy.strategy_id.clone(),
                strategy_type: snapshot.strategy.strategy_type.clone(),
                watchers: snapshot.strategy.watchers.clone(),
                trigger_rules: snapshot.strategy.trigger_rules.clone(),
                calculation_model: snapshot.strategy.calculation_model.clone(),
                action_templates: snapshot.strategy.action_templates.clone(),
                constraints: snapshot.strategy.constraints.clone(),
                unwind_rules: snapshot.strategy.unwind_rules.clone(),
            },
            rationale: snapshot.strategy.rationale.clone(),
            execution_preferences: snapshot.strategy.execution_preferences.clone(),
        })
        .map_err(|error| DaemonError::FastPathPreparation {
            reason: format_compiler_failure(&error),
        })?,
        runtime_state: snapshot.runtime_state,
        next_tick_at: snapshot.next_tick_at,
        last_event_id: snapshot.last_event_id,
        metrics: snapshot.metrics,
        watcher_states: snapshot
            .watcher_states
            .into_iter()
            .map(|watcher| RuntimeWatcherState {
                watcher_key: watcher.watcher_key,
                metric: watcher.metric,
                value: watcher.value,
                cursor: watcher.cursor,
                sampled_at: watcher.sampled_at,
            })
            .collect(),
        trigger_memory: snapshot
            .trigger_memory
            .into_iter()
            .map(|trigger| RuntimeTriggerMemory {
                trigger_key: trigger.trigger_key,
                cooldown_until: trigger.cooldown_until,
                last_fired_at: trigger.last_fired_at,
                hysteresis_armed: trigger.hysteresis_armed,
            })
            .collect(),
        pending_hedge: snapshot.pending_hedge.map(|hedge| RuntimePendingHedge {
            venue: hedge.venue,
            instrument: hedge.instrument,
            client_order_id: hedge.client_order_id,
            signer_address: hedge.signer_address,
            account_address: hedge.account_address,
            order_id: hedge.order_id,
            nonce: hedge.nonce,
            status: hedge.status,
            last_synced_at: hedge.last_synced_at,
        }),
    })
}

fn persisted_runtime_snapshot(
    strategy: a2ex_state::PersistedStrategyRegistration,
    snapshot: StrategyRuntimeSnapshot,
) -> PersistedStrategyRecoverySnapshot {
    PersistedStrategyRecoverySnapshot {
        strategy,
        runtime_state: snapshot.runtime_state,
        next_tick_at: snapshot.next_tick_at.clone(),
        last_event_id: snapshot.last_event_id,
        metrics: snapshot.metrics,
        watcher_states: snapshot
            .watcher_states
            .into_iter()
            .map(|watcher| PersistedWatcherState {
                watcher_key: watcher.watcher_key,
                metric: watcher.metric,
                value: watcher.value,
                cursor: watcher.cursor,
                sampled_at: watcher.sampled_at,
            })
            .collect(),
        trigger_memory: snapshot
            .trigger_memory
            .into_iter()
            .map(|trigger| PersistedTriggerMemory {
                trigger_key: trigger.trigger_key,
                cooldown_until: trigger.cooldown_until,
                last_fired_at: trigger.last_fired_at,
                hysteresis_armed: trigger.hysteresis_armed,
            })
            .collect(),
        pending_hedge: snapshot.pending_hedge.map(|hedge| PersistedPendingHedge {
            venue: hedge.venue,
            instrument: hedge.instrument,
            client_order_id: hedge.client_order_id,
            signer_address: hedge.signer_address,
            account_address: hedge.account_address,
            order_id: hedge.order_id,
            nonce: hedge.nonce,
            status: hedge.status,
            last_synced_at: hedge.last_synced_at,
        }),
        updated_at: snapshot
            .next_tick_at
            .unwrap_or_else(|| "2026-03-11T00:00:00Z".to_owned()),
    }
}

pub fn project_route_support_truth(
    capability_matrix: &CapabilityMatrix,
    compiled: &CompiledIntent,
    plan: Option<&ExecutionPlan>,
    reserved_capital_usd: Option<u64>,
) -> RouteSupportTruth {
    let approval_requirements = approval_requirements_for_preview(
        capability_matrix,
        plan,
        &compiled.constraints.allowed_venues,
        &compiled.funding.source_chain,
    );
    let capital_support = capital_support_for_route(compiled, reserved_capital_usd);

    RouteSupportTruth {
        capital_support,
        approval_requirements,
    }
}

fn approval_requirements_for_preview(
    capability_matrix: &CapabilityMatrix,
    plan: Option<&ExecutionPlan>,
    fallback_venues: &[String],
    source_chain: &str,
) -> Vec<PreviewApprovalRequirement> {
    let mut venues = BTreeMap::<String, PreviewApprovalRequirement>::new();

    if let Some(plan) = plan {
        for step in &plan.steps {
            if !step.approval_required {
                continue;
            }
            if let Some(capability) = capability_matrix.venue(&step.adapter) {
                for requirement in &capability.approval_requirements {
                    let chain = approval_chain_for_preview(
                        &step.adapter,
                        source_chain,
                        capability.supported_chains.as_slice(),
                    );
                    let key = format!(
                        "{}:{}:{}:{}",
                        step.adapter,
                        requirement.approval_type,
                        requirement.asset.as_deref().unwrap_or("none"),
                        chain.as_deref().unwrap_or("none")
                    );
                    venues
                        .entry(key)
                        .or_insert_with(|| PreviewApprovalRequirement {
                            venue: step.adapter.clone(),
                            approval_type: requirement.approval_type.clone(),
                            asset: requirement.asset.clone(),
                            chain,
                            context: requirement.context.clone(),
                            required: requirement.required,
                            auth_summary: capability.auth_summary.clone(),
                            summary: requirement.summary.clone(),
                        });
                }
            }
        }
    }

    for venue in fallback_venues {
        if let Some(capability) = capability_matrix.venue(venue) {
            for requirement in &capability.approval_requirements {
                let chain = approval_chain_for_preview(
                    venue,
                    source_chain,
                    capability.supported_chains.as_slice(),
                );
                let key = format!(
                    "{}:{}:{}:{}",
                    venue,
                    requirement.approval_type,
                    requirement.asset.as_deref().unwrap_or("none"),
                    chain.as_deref().unwrap_or("none")
                );
                venues
                    .entry(key)
                    .or_insert_with(|| PreviewApprovalRequirement {
                        venue: venue.clone(),
                        approval_type: requirement.approval_type.clone(),
                        asset: requirement.asset.clone(),
                        chain,
                        context: requirement.context.clone(),
                        required: requirement.required,
                        auth_summary: capability.auth_summary.clone(),
                        summary: requirement.summary.clone(),
                    });
            }
        }
    }

    venues.into_values().collect()
}

fn capital_support_for_route(
    compiled: &CompiledIntent,
    reserved_capital_usd: Option<u64>,
) -> RouteCapitalSupport {
    let required_capital_usd = compiled.objective.target_notional_usd;
    let completeness =
        if reserved_capital_usd.is_some_and(|reserved| reserved >= required_capital_usd) {
            a2ex_skill_bundle::ProposalQuantitativeCompleteness::Complete
        } else {
            a2ex_skill_bundle::ProposalQuantitativeCompleteness::Unknown
        };
    let summary = match reserved_capital_usd {
        Some(reserved) if reserved >= required_capital_usd => format!(
            "Route requires ${required_capital_usd} in {} and the same amount is already reserved locally.",
            compiled.funding.preferred_asset
        ),
        Some(reserved) => format!(
            "Route requires ${required_capital_usd} in {} but only ${reserved} is reserved locally.",
            compiled.funding.preferred_asset
        ),
        None => format!(
            "Route requires ${required_capital_usd} in {} before execution can begin.",
            compiled.funding.preferred_asset
        ),
    };
    let reason = match reserved_capital_usd {
        Some(reserved) if reserved >= required_capital_usd => format!(
            "Reserved {} capital already covers the required route notional.",
            compiled.funding.preferred_asset
        ),
        Some(reserved) => format!(
            "Only ${reserved} of required {} capital is reserved locally and live available balance evidence remains unknown.",
            compiled.funding.preferred_asset
        ),
        None => format!(
            "The proposal contract does not provide available {} bankroll truth and no held reservation exists yet, so route readiness keeps capital sufficiency unknown instead of guessing.",
            compiled.funding.preferred_asset
        ),
    };

    RouteCapitalSupport {
        required_capital_usd,
        available_capital_usd: None,
        reserved_capital_usd,
        completeness,
        summary,
        reason,
    }
}

fn approval_chain_for_preview(
    venue: &str,
    source_chain: &str,
    supported_chains: &[String],
) -> Option<String> {
    if venue == "across" && supported_chains.iter().any(|chain| chain == source_chain) {
        return Some(source_chain.to_owned());
    }
    None
}

fn reconcile_plan_execution(
    plan: &PersistedExecutionPlan,
    steps: &[PersistedExecutionPlanStep],
    reconciled_at: &str,
) -> ExecutionReconciliationReport {
    let mut balances = Vec::new();
    let mut fills = Vec::new();
    let mut positions = Vec::new();

    for step in &plan.plan.steps {
        let Some(persisted) = steps
            .iter()
            .find(|candidate| candidate.step_id == step.step_id)
        else {
            continue;
        };
        let metadata = step_metadata_value(persisted);

        match &step.params {
            PlanStepParams::Bridge(params) => {
                let actual_amount_usd = if persisted.status == "settled" {
                    params.amount_usd
                } else {
                    0
                };
                balances.push(BalanceReconciliationItem {
                    step_id: step.step_id.clone(),
                    asset: params.asset.clone(),
                    expected_amount_usd: params.amount_usd,
                    actual_amount_usd,
                    delta_usd: params.amount_usd as i64 - actual_amount_usd as i64,
                    observed_status: metadata
                        .get("bridge_status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&persisted.status)
                        .to_owned(),
                });
            }
            PlanStepParams::Entry(params) => {
                let actual_fill_usd = metadata
                    .get("filled_amount")
                    .and_then(|value| match value {
                        serde_json::Value::String(s) => s.parse::<u64>().ok(),
                        serde_json::Value::Number(n) => n.as_u64(),
                        _ => None,
                    })
                    .unwrap_or(0);
                fills.push(FillReconciliationItem {
                    step_id: step.step_id.clone(),
                    venue: persisted.adapter.clone(),
                    market: params.market.clone(),
                    expected_fill_usd: params.notional_usd,
                    actual_fill_usd,
                    delta_usd: params.notional_usd as i64 - actual_fill_usd as i64,
                    observed_status: metadata
                        .get("order_status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&persisted.status)
                        .to_owned(),
                });
            }
            PlanStepParams::Hedge(params) => {
                let actual_position_usd =
                    position_value_for_instrument(metadata.get("positions"), &params.instrument)
                        .unwrap_or(0);
                positions.push(PositionReconciliationItem {
                    step_id: step.step_id.clone(),
                    venue: persisted.adapter.clone(),
                    instrument: params.instrument.clone(),
                    expected_position_usd: params.notional_usd,
                    actual_position_usd,
                    delta_usd: params.notional_usd as i64 - actual_position_usd as i64,
                    observed_status: metadata
                        .get("order_status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&persisted.status)
                        .to_owned(),
                });
            }
        }
    }

    let residual_exposure_usd = fills.iter().map(|item| item.delta_usd.abs()).sum::<i64>()
        + positions
            .iter()
            .map(|item| item.delta_usd.abs())
            .sum::<i64>();
    let rebalance_required = residual_exposure_usd > 0
        || balances.iter().any(|item| item.delta_usd != 0)
        || fills.iter().any(|item| item.delta_usd != 0)
        || positions.iter().any(|item| item.delta_usd != 0);

    ExecutionReconciliationReport {
        execution_id: plan.plan_id.clone(),
        plan_id: plan.plan_id.clone(),
        balances,
        fills,
        positions,
        residual_exposure_usd,
        rebalance_required,
        reconciled_at: reconciled_at.to_owned(),
    }
}

fn step_metadata_value(step: &PersistedExecutionPlanStep) -> serde_json::Value {
    step.metadata_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn position_value_for_instrument(
    positions: Option<&serde_json::Value>,
    instrument: &str,
) -> Option<u64> {
    positions
        .and_then(serde_json::Value::as_array)
        .and_then(|entries| {
            entries.iter().find_map(|entry| {
                if entry.get("instrument").and_then(serde_json::Value::as_str) != Some(instrument) {
                    return None;
                }
                entry
                    .get("position_value")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| value.parse::<f64>().ok())
                    .map(|value| value.round() as u64)
            })
        })
}

fn stateful_hedge_identities(strategy_id: &str) -> (String, String) {
    (
        format!("hl-signer-{strategy_id}"),
        format!("hl-account-{strategy_id}"),
    )
}

fn seed_plan_step(
    plan_id: &str,
    step: &PlanStep,
    status: &str,
    attempts: u32,
    last_error: Option<String>,
    metadata_json: Option<String>,
    updated_at: &str,
) -> PersistedExecutionPlanStep {
    PersistedExecutionPlanStep {
        plan_id: plan_id.to_owned(),
        step_id: step.step_id.clone(),
        sequence_no: step.sequence,
        step_type: step.step_type.clone(),
        adapter: step.adapter.clone(),
        idempotency_key: step.idempotency_key.clone(),
        status: status.to_owned(),
        attempts,
        last_error,
        metadata_json,
        updated_at: updated_at.to_owned(),
    }
}

fn is_terminal_step_status(status: &str) -> bool {
    matches!(status, "settled" | "filled" | "confirmed")
}

fn prediction_venue(adapter: &str) -> Result<PredictionVenue, DaemonError> {
    match adapter {
        "polymarket" => Ok(PredictionVenue::Polymarket),
        "kalshi" => Ok(PredictionVenue::Kalshi),
        other => Err(DaemonError::PlanExecution {
            reason: format!("unsupported prediction venue {other}"),
        }),
    }
}

fn prediction_auth(venue: PredictionVenue) -> PredictionAuth {
    PredictionAuth {
        credential_id: format!("{}-local-auth", venue.as_str()),
        auth_summary: match venue {
            PredictionVenue::Polymarket => "locally derived Polymarket order auth".to_owned(),
            PredictionVenue::Kalshi => "locally signed Kalshi API auth".to_owned(),
        },
    }
}

fn result_with_fallback_status<E>(
    result: Result<String, E>,
    terminal_status: &str,
) -> Result<String, E> {
    result.map(|status| {
        if matches!(status.as_str(), "settled" | "filled" | "confirmed") {
            status
        } else {
            terminal_status.to_owned()
        }
    })
}

fn adapter_native_report(
    prepared: &a2ex_hyperliquid_adapter::PreparedHyperliquidOrder,
    order_id: Option<u64>,
    synced_status: &str,
) -> TxLifecycleReport {
    let lifecycle_status = match synced_status {
        "filled" | "confirmed" => TxLifecycleStatus::Confirmed,
        "failed" => TxLifecycleStatus::Failed,
        _ => TxLifecycleStatus::Pending,
    };
    TxLifecycleReport {
        prepared: PreparedEvmTransaction {
            chain_id: 0,
            to: "adapter://hyperliquid/order".to_owned(),
            value_wei: "0".to_owned(),
            calldata: prepared.client_order_id.clone().into_bytes(),
        },
        events: vec![a2ex_evm_adapter::TxLifecycleEvent {
            status: lifecycle_status,
            metadata: a2ex_evm_adapter::TxReceiptMetadata {
                chain_id: 0,
                tx_hash: order_id
                    .map(|value| format!("hl-order-{value}"))
                    .unwrap_or_else(|| prepared.client_order_id.clone()),
                confirmation_depth: 0,
                block_number: None,
                receipt_status: synced_status.to_owned(),
                error: None,
            },
        }],
    }
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

fn prepared_action_into_evm_transaction(action: &PreparedFastAction) -> PreparedEvmTransaction {
    match &action.payload {
        PreparedVenueAction::GenericContractCall {
            chain_id,
            to,
            value_wei,
            calldata,
            ..
        } => PreparedEvmTransaction {
            chain_id: *chain_id,
            to: to.clone(),
            value_wei: value_wei.clone(),
            calldata: calldata.clone(),
        },
        PreparedVenueAction::SimpleEntry {
            venue,
            market,
            side,
            notional_usd,
        } => PreparedEvmTransaction {
            chain_id: 8453,
            to: format!("entry://{venue}/{market}"),
            value_wei: "0".to_owned(),
            calldata: format!("simple_entry:{side}:{notional_usd}").into_bytes(),
        },
        PreparedVenueAction::HedgeAdjustPrecomputed {
            venue,
            instrument,
            target_delta_bps,
            notional_usd,
        } => PreparedEvmTransaction {
            chain_id: 8453,
            to: format!("hedge://{venue}/{instrument}"),
            value_wei: "0".to_owned(),
            calldata: format!("hedge_adjust:{target_delta_bps}:{notional_usd}").into_bytes(),
        },
    }
}

fn tx_status_label(status: &TxLifecycleStatus) -> &'static str {
    match status {
        TxLifecycleStatus::Prepared => "prepared",
        TxLifecycleStatus::Submitted => "submitted",
        TxLifecycleStatus::Pending => "pending",
        TxLifecycleStatus::Confirmed => "confirmed",
        TxLifecycleStatus::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_local_control_requests_for_policy_evaluation() {
        let endpoint = LocalControlEndpoint::new("/tmp/a2ex-control.sock");
        let request = JsonRpcRequest::new(
            "req-1",
            DAEMON_CONTROL_METHOD,
            ExecutionRequest {
                action_id: "exec-1".to_owned(),
                action_kind: "submit_intent".to_owned(),
                notional_usd: 25,
                reservation_id: "reservation-1".to_owned(),
            },
        );

        let decoded = decode_control_request(&endpoint, request).expect("request decodes");

        assert_eq!(decoded.transport, endpoint.transport());
        assert_eq!(decoded.request.action_kind, "submit_intent");
    }
}
