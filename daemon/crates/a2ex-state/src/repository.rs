use std::path::{Path, PathBuf};

use a2ex_control::{
    ActionTemplate, AgentRequestEnvelope, CalculationModel, ExecutionPreferences, Intent,
    RationaleSummary, RouteDecision, Strategy, StrategyConstraints, TriggerRule, UnwindRule,
    WatcherSpec,
};
use a2ex_planner::ExecutionPlan;
use a2ex_skill_bundle::{BundleDiagnostic, InterpretationBlocker};
use rusqlite::{OptionalExtension, params, params_from_iter};
use thiserror::Error;
use tokio_rusqlite::Connection;
use uuid::Uuid;

use crate::reconciliation::{
    CanonicalStateSnapshot, ExecutionStateRecord, ReconciliationStateRecord,
    StrategyRuntimeStateRecord,
};
use crate::schema::BOOTSTRAP_SQL;

const STRATEGY_EVENT_TYPE: &str = "strategy_state_changed";
const STRATEGY_REGISTRATION_EVENT_TYPE: &str = "strategy_registered";
const EXECUTION_EVENT_TYPE: &str = "execution_state_changed";
const RECONCILIATION_EVENT_TYPE: &str = "reconciliation_state_changed";
const INTENT_EVENT_TYPE: &str = "intent_submitted";
const ROUTE_DECISION_EVENT_TYPE: &str = "route_decision_recorded";
const PLAN_CREATED_EVENT_TYPE: &str = "execution_plan_created";
const PLAN_STEP_EVENT_TYPE: &str = "execution_plan_step_state_changed";
const RUNTIME_CONTROL_EVENT_TYPE: &str = "runtime_control_changed";
const STRATEGY_SELECTION_MATERIALIZED_EVENT_TYPE: &str = "strategy_selection_materialized";
const STRATEGY_SELECTION_OVERRIDE_APPLIED_EVENT_TYPE: &str = "strategy_selection_override_applied";
const STRATEGY_SELECTION_APPROVED_EVENT_TYPE: &str = "strategy_selection_approved";
const STRATEGY_SELECTION_REOPENED_EVENT_TYPE: &str = "strategy_selection_reopened";
const STRATEGY_RUNTIME_STREAM_TYPE: &str = "strategy_runtime_handoff";
const STRATEGY_RUNTIME_HANDOFF_EVENT_TYPE: &str = "strategy_runtime_handoff_persisted";
const STRATEGY_RUNTIME_ELIGIBILITY_EVENT_TYPE: &str = "strategy_runtime_eligibility_changed";
const STRATEGY_RUNTIME_IDENTITY_REFRESHED_EVENT_TYPE: &str = "strategy_runtime_identity_refreshed";

pub const AUTONOMOUS_RUNTIME_CONTROL_SCOPE: &str = "autonomous_runtime";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub event_id: String,
    pub stream_type: String,
    pub stream_id: String,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: String,
}

impl JournalEntry {
    pub fn event_type(&self) -> &str {
        &self.event_type
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedOnboardingWorkspace {
    pub workspace_id: String,
    pub canonical_workspace_root: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedOnboardingInstall {
    pub install_id: String,
    pub workspace_id: String,
    pub install_url: String,
    pub attached_bundle_url: String,
    pub claim_disposition: String,
    pub bootstrap_source: String,
    pub bootstrap_path: String,
    pub state_db_path: String,
    pub analytics_db_path: String,
    pub used_remote_control_plane: bool,
    pub recovered_existing_state: bool,
    pub bootstrap_attempt_count: u32,
    pub last_bootstrap_attempt_at: Option<String>,
    pub last_bootstrap_completed_at: Option<String>,
    pub last_bootstrap_failure_stage: Option<String>,
    pub last_bootstrap_failure_summary: Option<String>,
    pub readiness_status: String,
    pub readiness_blockers: Vec<InterpretationBlocker>,
    pub readiness_diagnostics: Vec<BundleDiagnostic>,
    pub onboarding_status: String,
    pub bundle_drift: Option<serde_json::Value>,
    pub last_onboarding_rejection_code: Option<String>,
    pub last_onboarding_rejection_message: Option<String>,
    pub last_onboarding_rejection_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedOnboardingChecklistItem {
    pub install_id: String,
    pub checklist_key: String,
    pub source_kind: String,
    pub status: String,
    pub blocker_reason: Option<String>,
    pub next_action: Option<String>,
    pub evidence: serde_json::Value,
    pub lifecycle: Option<serde_json::Value>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedRouteReadiness {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub request_id: String,
    pub status: String,
    pub capital: serde_json::Value,
    pub approvals: serde_json::Value,
    pub blockers: serde_json::Value,
    pub recommended_owner_action: Option<serde_json::Value>,
    pub ordered_steps: serde_json::Value,
    pub current_step_key: Option<String>,
    pub last_route_rejection_code: Option<String>,
    pub last_route_rejection_message: Option<String>,
    pub last_route_rejection_at: Option<String>,
    pub evaluation: Option<serde_json::Value>,
    pub evaluation_fingerprint: Option<String>,
    pub stale_status: String,
    pub stale_reason: Option<String>,
    pub stale_detected_at: Option<String>,
    pub evaluated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedRouteReadinessStep {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub step_key: String,
    pub status: String,
    pub blocker_reason: Option<String>,
    pub recommended_action: Option<serde_json::Value>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedStrategySelection {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
    pub reopened_from_revision: Option<u32>,
    pub proposal_revision: i64,
    pub proposal_uri: String,
    pub proposal_snapshot: serde_json::Value,
    pub recommendation_basis: serde_json::Value,
    pub readiness_sensitivity_summary: serde_json::Value,
    pub approval: serde_json::Value,
    pub approval_stale: bool,
    pub approval_stale_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedStrategySelectionApprovalHistoryEvent {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub event_kind: String,
    pub selection_revision: u32,
    pub approved_revision: Option<u32>,
    pub reopened_from_revision: Option<u32>,
    pub approved_by: Option<String>,
    pub note: Option<String>,
    pub reason: Option<String>,
    pub provenance: Option<serde_json::Value>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedStrategySelectionOverride {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub override_key: String,
    pub previous_value: serde_json::Value,
    pub new_value: serde_json::Value,
    pub rationale: String,
    pub provenance: serde_json::Value,
    pub sensitivity_class: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedStrategyRuntimeHandoff {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub approved_selection_revision: u32,
    pub route_id: String,
    pub request_id: String,
    pub route_readiness_fingerprint: String,
    pub route_readiness_status: String,
    pub route_readiness_evaluated_at: String,
    pub eligibility_status: String,
    pub hold_reason: Option<String>,
    pub runtime_control_mode: String,
    pub strategy_id: Option<String>,
    pub runtime_identity_refreshed_at: Option<String>,
    pub runtime_identity_source: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedRuntimeControl {
    pub scope_key: String,
    pub control_mode: String,
    pub transition_reason: String,
    pub transition_source: String,
    pub transitioned_at: String,
    pub last_cleared_at: Option<String>,
    pub last_cleared_reason: Option<String>,
    pub last_cleared_source: Option<String>,
    pub last_rejection_code: Option<String>,
    pub last_rejection_message: Option<String>,
    pub last_rejection_operation: Option<String>,
    pub last_rejection_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedCapitalReservation {
    pub reservation_id: String,
    pub execution_id: String,
    pub asset: String,
    pub amount: u64,
    pub state: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedIntentSubmission {
    pub request_id: String,
    pub intent_id: String,
    pub source_agent_id: String,
    pub intent_type: String,
    pub rationale: RationaleSummary,
    pub execution_preferences: ExecutionPreferences,
    pub submitted_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedStrategyRegistration {
    pub request_id: String,
    pub strategy_id: String,
    pub source_agent_id: String,
    pub strategy_type: String,
    pub watchers: Vec<WatcherSpec>,
    pub trigger_rules: Vec<TriggerRule>,
    pub calculation_model: CalculationModel,
    pub action_templates: Vec<ActionTemplate>,
    pub constraints: StrategyConstraints,
    pub unwind_rules: Vec<UnwindRule>,
    pub rationale: RationaleSummary,
    pub execution_preferences: ExecutionPreferences,
    pub submitted_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRouteDecision {
    pub request_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub route: RouteDecision,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedExecutionPlan {
    pub plan_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub request_id: String,
    pub status: String,
    pub summary: String,
    pub plan: ExecutionPlan,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedExecutionPlanStep {
    pub plan_id: String,
    pub step_id: String,
    pub sequence_no: u32,
    pub step_type: String,
    pub adapter: String,
    pub idempotency_key: String,
    pub status: String,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub metadata_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecutionLifecyclePayload {
    pub execution_id: String,
    pub plan_id: String,
    pub status: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RouteDecisionPayload {
    request_id: String,
    source_kind: String,
    source_id: String,
    route: RouteDecision,
}

#[derive(Debug)]
struct RawPersistedIntentSubmission {
    request_id: String,
    intent_id: String,
    source_agent_id: String,
    intent_type: String,
    rationale_json: String,
    execution_prefs_json: String,
    submitted_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct RawPersistedStrategyRegistration {
    request_id: String,
    strategy_id: String,
    source_agent_id: String,
    payload_json: String,
    rationale_json: String,
    execution_prefs_json: String,
    submitted_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedWatcherState {
    pub watcher_key: String,
    pub metric: String,
    pub value: f64,
    pub cursor: String,
    pub sampled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedTriggerMemory {
    pub trigger_key: String,
    pub cooldown_until: Option<String>,
    pub last_fired_at: Option<String>,
    pub hysteresis_armed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedPendingHedge {
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

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedStrategyRecoverySnapshot {
    pub strategy: PersistedStrategyRegistration,
    pub runtime_state: String,
    pub next_tick_at: Option<String>,
    pub last_event_id: Option<String>,
    pub metrics: serde_json::Value,
    pub watcher_states: Vec<PersistedWatcherState>,
    pub trigger_memory: Vec<PersistedTriggerMemory>,
    pub pending_hedge: Option<PersistedPendingHedge>,
    pub updated_at: String,
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("failed to open sqlite state database at {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("failed to initialize sqlite state database at {path}")]
    Initialize {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("failed to persist canonical state at {path}")]
    Persist {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("failed to load canonical state at {path}")]
    Load {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("failed to load event journal at {path}")]
    LoadJournal {
        path: PathBuf,
        #[source]
        source: tokio_rusqlite::Error,
    },
    #[error("failed to serialize state payload at {path}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to decode state payload at {path}")]
    Deserialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug)]
pub struct StateRepository {
    path: PathBuf,
    connection: Connection,
}

impl StateRepository {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StateError> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path)
            .await
            .map_err(|source| StateError::Open {
                path: path.clone(),
                source,
            })?;

        connection
            .call(|conn| {
                conn.execute_batch(BOOTSTRAP_SQL)?;
                ensure_strategy_pending_hedge_columns(conn)?;
                ensure_onboarding_install_columns(conn)?;
                ensure_onboarding_route_readiness_schema(conn)?;
                ensure_strategy_selection_schema(conn)?;
                ensure_strategy_runtime_handoff_schema(conn)?;
                ensure_runtime_control_schema(conn)?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Initialize {
                path: path.clone(),
                source,
            })?;

        Ok(Self { path, connection })
    }

    pub async fn load_onboarding_workspace_by_root(
        &self,
        canonical_workspace_root: &str,
    ) -> Result<Option<PersistedOnboardingWorkspace>, StateError> {
        let path = self.path.clone();
        let canonical_workspace_root = canonical_workspace_root.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT workspace_id, canonical_workspace_root, created_at, updated_at
                     FROM onboarding_workspaces
                     WHERE canonical_workspace_root = ?1",
                    [canonical_workspace_root],
                    |row| {
                        Ok(PersistedOnboardingWorkspace {
                            workspace_id: row.get(0)?,
                            canonical_workspace_root: row.get(1)?,
                            created_at: row.get(2)?,
                            updated_at: row.get(3)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn persist_onboarding_workspace(
        &self,
        workspace: &PersistedOnboardingWorkspace,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let workspace = workspace.clone();

        self.connection
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO onboarding_workspaces (
                        workspace_id, canonical_workspace_root, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(workspace_id) DO UPDATE SET
                        canonical_workspace_root = excluded.canonical_workspace_root,
                        updated_at = excluded.updated_at",
                    params![
                        workspace.workspace_id,
                        workspace.canonical_workspace_root,
                        workspace.created_at,
                        workspace.updated_at,
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_onboarding_install(
        &self,
        workspace_id: &str,
        install_url: &str,
    ) -> Result<Option<PersistedOnboardingInstall>, StateError> {
        let path = self.path.clone();
        let workspace_id = workspace_id.to_owned();
        let install_url = install_url.to_owned();

        let raw = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT install_id, workspace_id, install_url, attached_bundle_url, claim_disposition,
                            bootstrap_source, bootstrap_path, state_db_path, analytics_db_path,
                            used_remote_control_plane, recovered_existing_state, bootstrap_attempt_count,
                            last_bootstrap_attempt_at, last_bootstrap_completed_at,
                            last_bootstrap_failure_stage, last_bootstrap_failure_summary,
                            readiness_status, readiness_blockers_json, readiness_diagnostics_json,
                            onboarding_status, bundle_drift_json,
                            last_onboarding_rejection_code, last_onboarding_rejection_message,
                            last_onboarding_rejection_at,
                            created_at, updated_at
                     FROM onboarding_installs
                     WHERE workspace_id = ?1 AND install_url = ?2",
                    params![workspace_id, install_url],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, i64>(9)? != 0,
                            row.get::<_, i64>(10)? != 0,
                            row.get::<_, u32>(11)?,
                            row.get::<_, Option<String>>(12)?,
                            row.get::<_, Option<String>>(13)?,
                            row.get::<_, Option<String>>(14)?,
                            row.get::<_, Option<String>>(15)?,
                            row.get::<_, String>(16)?,
                            row.get::<_, String>(17)?,
                            row.get::<_, String>(18)?,
                            row.get::<_, String>(19)?,
                            row.get::<_, Option<String>>(20)?,
                            row.get::<_, Option<String>>(21)?,
                            row.get::<_, Option<String>>(22)?,
                            row.get::<_, Option<String>>(23)?,
                            row.get::<_, String>(24)?,
                            row.get::<_, String>(25)?,
                        ))
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        raw.map(|raw| {
            let readiness_blockers =
                serde_json::from_str(&raw.17).map_err(|source| StateError::Deserialize {
                    path: path.clone(),
                    source,
                })?;
            let readiness_diagnostics =
                serde_json::from_str(&raw.18).map_err(|source| StateError::Deserialize {
                    path: path.clone(),
                    source,
                })?;
            let bundle_drift = raw
                .20
                .as_deref()
                .filter(|value| !value.is_empty() && *value != "null")
                .map(|value| {
                    serde_json::from_str(value).map_err(|source| StateError::Deserialize {
                        path: path.clone(),
                        source,
                    })
                })
                .transpose()?;

            Ok(PersistedOnboardingInstall {
                install_id: raw.0,
                workspace_id: raw.1,
                install_url: raw.2,
                attached_bundle_url: raw.3,
                claim_disposition: raw.4,
                bootstrap_source: raw.5,
                bootstrap_path: raw.6,
                state_db_path: raw.7,
                analytics_db_path: raw.8,
                used_remote_control_plane: raw.9,
                recovered_existing_state: raw.10,
                bootstrap_attempt_count: raw.11,
                last_bootstrap_attempt_at: raw.12,
                last_bootstrap_completed_at: raw.13,
                last_bootstrap_failure_stage: raw.14,
                last_bootstrap_failure_summary: raw.15,
                readiness_status: raw.16,
                readiness_blockers,
                readiness_diagnostics,
                onboarding_status: raw.19,
                bundle_drift,
                last_onboarding_rejection_code: raw.21,
                last_onboarding_rejection_message: raw.22,
                last_onboarding_rejection_at: raw.23,
                created_at: raw.24,
                updated_at: raw.25,
            })
        })
        .transpose()
    }

    pub async fn persist_onboarding_install(
        &self,
        install: &PersistedOnboardingInstall,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let install = install.clone();
        let readiness_blockers_json =
            serde_json::to_string(&install.readiness_blockers).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let readiness_diagnostics_json = serde_json::to_string(&install.readiness_diagnostics)
            .map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let bundle_drift_json = install
            .bundle_drift
            .as_ref()
            .map(|value| {
                serde_json::to_string(value).map_err(|source| StateError::Serialize {
                    path: path.clone(),
                    source,
                })
            })
            .transpose()?;
        let last_onboarding_rejection_code = install.last_onboarding_rejection_code.clone();
        let last_onboarding_rejection_message = install.last_onboarding_rejection_message.clone();
        let last_onboarding_rejection_at = install.last_onboarding_rejection_at.clone();

        self.connection
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO onboarding_installs (
                        install_id, workspace_id, install_url, attached_bundle_url, claim_disposition,
                        bootstrap_source, bootstrap_path, state_db_path, analytics_db_path,
                        used_remote_control_plane, recovered_existing_state, bootstrap_attempt_count,
                        last_bootstrap_attempt_at, last_bootstrap_completed_at,
                        last_bootstrap_failure_stage, last_bootstrap_failure_summary,
                        readiness_status, readiness_blockers_json, readiness_diagnostics_json,
                        onboarding_status, bundle_drift_json,
                        last_onboarding_rejection_code, last_onboarding_rejection_message,
                        last_onboarding_rejection_at,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
                     ON CONFLICT(install_id) DO UPDATE SET
                        workspace_id = excluded.workspace_id,
                        install_url = excluded.install_url,
                        attached_bundle_url = excluded.attached_bundle_url,
                        claim_disposition = excluded.claim_disposition,
                        bootstrap_source = excluded.bootstrap_source,
                        bootstrap_path = excluded.bootstrap_path,
                        state_db_path = excluded.state_db_path,
                        analytics_db_path = excluded.analytics_db_path,
                        used_remote_control_plane = excluded.used_remote_control_plane,
                        recovered_existing_state = excluded.recovered_existing_state,
                        bootstrap_attempt_count = excluded.bootstrap_attempt_count,
                        last_bootstrap_attempt_at = excluded.last_bootstrap_attempt_at,
                        last_bootstrap_completed_at = excluded.last_bootstrap_completed_at,
                        last_bootstrap_failure_stage = excluded.last_bootstrap_failure_stage,
                        last_bootstrap_failure_summary = excluded.last_bootstrap_failure_summary,
                        readiness_status = excluded.readiness_status,
                        readiness_blockers_json = excluded.readiness_blockers_json,
                        readiness_diagnostics_json = excluded.readiness_diagnostics_json,
                        onboarding_status = excluded.onboarding_status,
                        bundle_drift_json = excluded.bundle_drift_json,
                        last_onboarding_rejection_code = excluded.last_onboarding_rejection_code,
                        last_onboarding_rejection_message = excluded.last_onboarding_rejection_message,
                        last_onboarding_rejection_at = excluded.last_onboarding_rejection_at,
                        updated_at = excluded.updated_at",
                    params![
                        install.install_id,
                        install.workspace_id,
                        install.install_url,
                        install.attached_bundle_url,
                        install.claim_disposition,
                        install.bootstrap_source,
                        install.bootstrap_path,
                        install.state_db_path,
                        install.analytics_db_path,
                        if install.used_remote_control_plane { 1 } else { 0 },
                        if install.recovered_existing_state { 1 } else { 0 },
                        install.bootstrap_attempt_count,
                        install.last_bootstrap_attempt_at,
                        install.last_bootstrap_completed_at,
                        install.last_bootstrap_failure_stage,
                        install.last_bootstrap_failure_summary,
                        install.readiness_status,
                        readiness_blockers_json,
                        readiness_diagnostics_json,
                        install.onboarding_status,
                        bundle_drift_json,
                        last_onboarding_rejection_code,
                        last_onboarding_rejection_message,
                        last_onboarding_rejection_at,
                        install.created_at,
                        install.updated_at,
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_onboarding_install_by_id(
        &self,
        install_id: &str,
    ) -> Result<Option<PersistedOnboardingInstall>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();

        let raw = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT install_id, workspace_id, install_url, attached_bundle_url, claim_disposition,
                            bootstrap_source, bootstrap_path, state_db_path, analytics_db_path,
                            used_remote_control_plane, recovered_existing_state, bootstrap_attempt_count,
                            last_bootstrap_attempt_at, last_bootstrap_completed_at,
                            last_bootstrap_failure_stage, last_bootstrap_failure_summary,
                            readiness_status, readiness_blockers_json, readiness_diagnostics_json,
                            onboarding_status, bundle_drift_json,
                            last_onboarding_rejection_code, last_onboarding_rejection_message,
                            last_onboarding_rejection_at,
                            created_at, updated_at
                     FROM onboarding_installs
                     WHERE install_id = ?1",
                    [install_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, i64>(9)? != 0,
                            row.get::<_, i64>(10)? != 0,
                            row.get::<_, u32>(11)?,
                            row.get::<_, Option<String>>(12)?,
                            row.get::<_, Option<String>>(13)?,
                            row.get::<_, Option<String>>(14)?,
                            row.get::<_, Option<String>>(15)?,
                            row.get::<_, String>(16)?,
                            row.get::<_, String>(17)?,
                            row.get::<_, String>(18)?,
                            row.get::<_, String>(19)?,
                            row.get::<_, Option<String>>(20)?,
                            row.get::<_, Option<String>>(21)?,
                            row.get::<_, Option<String>>(22)?,
                            row.get::<_, Option<String>>(23)?,
                            row.get::<_, String>(24)?,
                            row.get::<_, String>(25)?,
                        ))
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        raw.map(|raw| {
            let readiness_blockers =
                serde_json::from_str(&raw.17).map_err(|source| StateError::Deserialize {
                    path: path.clone(),
                    source,
                })?;
            let readiness_diagnostics =
                serde_json::from_str(&raw.18).map_err(|source| StateError::Deserialize {
                    path: path.clone(),
                    source,
                })?;
            let bundle_drift = raw
                .20
                .as_deref()
                .filter(|value| !value.is_empty() && *value != "null")
                .map(|value| {
                    serde_json::from_str(value).map_err(|source| StateError::Deserialize {
                        path: path.clone(),
                        source,
                    })
                })
                .transpose()?;

            Ok(PersistedOnboardingInstall {
                install_id: raw.0,
                workspace_id: raw.1,
                install_url: raw.2,
                attached_bundle_url: raw.3,
                claim_disposition: raw.4,
                bootstrap_source: raw.5,
                bootstrap_path: raw.6,
                state_db_path: raw.7,
                analytics_db_path: raw.8,
                used_remote_control_plane: raw.9,
                recovered_existing_state: raw.10,
                bootstrap_attempt_count: raw.11,
                last_bootstrap_attempt_at: raw.12,
                last_bootstrap_completed_at: raw.13,
                last_bootstrap_failure_stage: raw.14,
                last_bootstrap_failure_summary: raw.15,
                readiness_status: raw.16,
                readiness_blockers,
                readiness_diagnostics,
                onboarding_status: raw.19,
                bundle_drift,
                last_onboarding_rejection_code: raw.21,
                last_onboarding_rejection_message: raw.22,
                last_onboarding_rejection_at: raw.23,
                created_at: raw.24,
                updated_at: raw.25,
            })
        })
        .transpose()
    }

    pub async fn load_onboarding_checklist_items(
        &self,
        install_id: &str,
    ) -> Result<Vec<PersistedOnboardingChecklistItem>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();

        self.connection
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT install_id, checklist_key, source_kind, status, blocker_reason, next_action,
                            evidence_json, lifecycle_json, completed_at, created_at, updated_at
                     FROM onboarding_checklist_items
                     WHERE install_id = ?1
                     ORDER BY checklist_key",
                )?;
                stmt.query_map([install_id], |row| {
                    let evidence_json: String = row.get(6)?;
                    let lifecycle_json: Option<String> = row.get(7)?;
                    Ok(PersistedOnboardingChecklistItem {
                        install_id: row.get(0)?,
                        checklist_key: row.get(1)?,
                        source_kind: row.get(2)?,
                        status: row.get(3)?,
                        blocker_reason: row.get(4)?,
                        next_action: row.get(5)?,
                        evidence: serde_json::from_str(&evidence_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                6,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                        lifecycle: lifecycle_json
                            .map(|value| {
                                serde_json::from_str(&value).map_err(|error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        7,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                })
                            })
                            .transpose()?,
                        completed_at: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_onboarding_install_state(
        &self,
        install_id: &str,
    ) -> Result<
        Option<(
            PersistedOnboardingInstall,
            Vec<PersistedOnboardingChecklistItem>,
        )>,
        StateError,
    > {
        let install = self.load_onboarding_install_by_id(install_id).await?;
        match install {
            Some(install) => {
                let items = self.load_onboarding_checklist_items(install_id).await?;
                Ok(Some((install, items)))
            }
            None => Ok(None),
        }
    }

    pub async fn replace_onboarding_checklist_items(
        &self,
        install_id: &str,
        items: &[PersistedOnboardingChecklistItem],
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let items = items.to_vec();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                replace_onboarding_checklist_items_tx(&tx, &install_id, &items)?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn persist_onboarding_install_state(
        &self,
        install: &PersistedOnboardingInstall,
        items: &[PersistedOnboardingChecklistItem],
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let install = install.clone();
        let items = items.to_vec();
        let readiness_blockers_json =
            serde_json::to_string(&install.readiness_blockers).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let readiness_diagnostics_json = serde_json::to_string(&install.readiness_diagnostics)
            .map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let bundle_drift_json = install
            .bundle_drift
            .as_ref()
            .map(|value| {
                serde_json::to_string(value).map_err(|source| StateError::Serialize {
                    path: path.clone(),
                    source,
                })
            })
            .transpose()?;
        let last_onboarding_rejection_code = install.last_onboarding_rejection_code.clone();
        let last_onboarding_rejection_message = install.last_onboarding_rejection_message.clone();
        let last_onboarding_rejection_at = install.last_onboarding_rejection_at.clone();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO onboarding_installs (
                        install_id, workspace_id, install_url, attached_bundle_url, claim_disposition,
                        bootstrap_source, bootstrap_path, state_db_path, analytics_db_path,
                        used_remote_control_plane, recovered_existing_state, bootstrap_attempt_count,
                        last_bootstrap_attempt_at, last_bootstrap_completed_at,
                        last_bootstrap_failure_stage, last_bootstrap_failure_summary,
                        readiness_status, readiness_blockers_json, readiness_diagnostics_json,
                        onboarding_status, bundle_drift_json,
                        last_onboarding_rejection_code, last_onboarding_rejection_message,
                        last_onboarding_rejection_at,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
                     ON CONFLICT(install_id) DO UPDATE SET
                        workspace_id = excluded.workspace_id,
                        install_url = excluded.install_url,
                        attached_bundle_url = excluded.attached_bundle_url,
                        claim_disposition = excluded.claim_disposition,
                        bootstrap_source = excluded.bootstrap_source,
                        bootstrap_path = excluded.bootstrap_path,
                        state_db_path = excluded.state_db_path,
                        analytics_db_path = excluded.analytics_db_path,
                        used_remote_control_plane = excluded.used_remote_control_plane,
                        recovered_existing_state = excluded.recovered_existing_state,
                        bootstrap_attempt_count = excluded.bootstrap_attempt_count,
                        last_bootstrap_attempt_at = excluded.last_bootstrap_attempt_at,
                        last_bootstrap_completed_at = excluded.last_bootstrap_completed_at,
                        last_bootstrap_failure_stage = excluded.last_bootstrap_failure_stage,
                        last_bootstrap_failure_summary = excluded.last_bootstrap_failure_summary,
                        readiness_status = excluded.readiness_status,
                        readiness_blockers_json = excluded.readiness_blockers_json,
                        readiness_diagnostics_json = excluded.readiness_diagnostics_json,
                        onboarding_status = excluded.onboarding_status,
                        bundle_drift_json = excluded.bundle_drift_json,
                        last_onboarding_rejection_code = excluded.last_onboarding_rejection_code,
                        last_onboarding_rejection_message = excluded.last_onboarding_rejection_message,
                        last_onboarding_rejection_at = excluded.last_onboarding_rejection_at,
                        updated_at = excluded.updated_at",
                    params![
                        install.install_id,
                        install.workspace_id,
                        install.install_url,
                        install.attached_bundle_url,
                        install.claim_disposition,
                        install.bootstrap_source,
                        install.bootstrap_path,
                        install.state_db_path,
                        install.analytics_db_path,
                        if install.used_remote_control_plane { 1 } else { 0 },
                        if install.recovered_existing_state { 1 } else { 0 },
                        install.bootstrap_attempt_count,
                        install.last_bootstrap_attempt_at,
                        install.last_bootstrap_completed_at,
                        install.last_bootstrap_failure_stage,
                        install.last_bootstrap_failure_summary,
                        install.readiness_status,
                        readiness_blockers_json,
                        readiness_diagnostics_json,
                        install.onboarding_status,
                        bundle_drift_json,
                        last_onboarding_rejection_code,
                        last_onboarding_rejection_message,
                        last_onboarding_rejection_at,
                        install.created_at,
                        install.updated_at,
                    ],
                )?;

                replace_onboarding_checklist_items_tx(&tx, &install.install_id, &items)?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_runtime_control(
        &self,
        scope_key: &str,
    ) -> Result<Option<PersistedRuntimeControl>, StateError> {
        let path = self.path.clone();
        let scope_key = scope_key.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT scope_key, control_mode, transition_reason, transition_source,
                            transitioned_at, last_cleared_at, last_cleared_reason,
                            last_cleared_source, last_rejection_code, last_rejection_message,
                            last_rejection_operation, last_rejection_at, updated_at
                     FROM runtime_control
                     WHERE scope_key = ?1",
                    [scope_key],
                    |row| {
                        Ok(PersistedRuntimeControl {
                            scope_key: row.get(0)?,
                            control_mode: row.get(1)?,
                            transition_reason: row.get(2)?,
                            transition_source: row.get(3)?,
                            transitioned_at: row.get(4)?,
                            last_cleared_at: row.get(5)?,
                            last_cleared_reason: row.get(6)?,
                            last_cleared_source: row.get(7)?,
                            last_rejection_code: row.get(8)?,
                            last_rejection_message: row.get(9)?,
                            last_rejection_operation: row.get(10)?,
                            last_rejection_at: row.get(11)?,
                            updated_at: row.get(12)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn persist_runtime_control(
        &self,
        record: &PersistedRuntimeControl,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let record = record.clone();
        let payload_json =
            serde_json::to_string(&record).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO runtime_control (
                        scope_key, control_mode, transition_reason, transition_source,
                        transitioned_at, last_cleared_at, last_cleared_reason,
                        last_cleared_source, last_rejection_code, last_rejection_message,
                        last_rejection_operation, last_rejection_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(scope_key) DO UPDATE SET
                        control_mode = excluded.control_mode,
                        transition_reason = excluded.transition_reason,
                        transition_source = excluded.transition_source,
                        transitioned_at = excluded.transitioned_at,
                        last_cleared_at = excluded.last_cleared_at,
                        last_cleared_reason = excluded.last_cleared_reason,
                        last_cleared_source = excluded.last_cleared_source,
                        last_rejection_code = excluded.last_rejection_code,
                        last_rejection_message = excluded.last_rejection_message,
                        last_rejection_operation = excluded.last_rejection_operation,
                        last_rejection_at = excluded.last_rejection_at,
                        updated_at = excluded.updated_at",
                    params![
                        record.scope_key,
                        record.control_mode,
                        record.transition_reason,
                        record.transition_source,
                        record.transitioned_at,
                        record.last_cleared_at,
                        record.last_cleared_reason,
                        record.last_cleared_source,
                        record.last_rejection_code,
                        record.last_rejection_message,
                        record.last_rejection_operation,
                        record.last_rejection_at,
                        record.updated_at,
                    ],
                )?;
                append_journal_entry(
                    &tx,
                    "runtime_control",
                    &record.scope_key,
                    RUNTIME_CONTROL_EVENT_TYPE,
                    payload_json,
                    &record.updated_at,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_capital_reservations_for_execution(
        &self,
        execution_id: &str,
    ) -> Result<Vec<PersistedCapitalReservation>, StateError> {
        let path = self.path.clone();
        let execution_id = execution_id.to_owned();

        self.connection
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT reservation_id, execution_id, asset, amount, state, updated_at
                     FROM capital_reservations
                     WHERE execution_id = ?1",
                )?;
                let rows = stmt.query_map(params![execution_id], |row| {
                    Ok(PersistedCapitalReservation {
                        reservation_id: row.get(0)?,
                        execution_id: row.get(1)?,
                        asset: row.get(2)?,
                        amount: row.get(3)?,
                        state: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_onboarding_route_readiness(
        &self,
        install_id: &str,
        proposal_id: &str,
        route_id: &str,
    ) -> Result<Option<PersistedRouteReadiness>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let proposal_id = proposal_id.to_owned();
        let route_id = route_id.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT install_id, proposal_id, route_id, request_id, status, capital_json,
                            approvals_json, blockers_json, recommended_owner_action_json,
                            ordered_steps_json, current_step_key,
                            last_route_rejection_code, last_route_rejection_message,
                            last_route_rejection_at, evaluation_json, evaluation_fingerprint,
                            stale_status, stale_reason, stale_detected_at,
                            evaluated_at, created_at, updated_at
                     FROM onboarding_route_readiness
                     WHERE install_id = ?1 AND proposal_id = ?2 AND route_id = ?3",
                    params![install_id, proposal_id, route_id],
                    |row| {
                        let capital_json: String = row.get(5)?;
                        let approvals_json: String = row.get(6)?;
                        let blockers_json: String = row.get(7)?;
                        let recommended_owner_action_json: Option<String> = row.get(8)?;
                        let ordered_steps_json: String = row.get(9)?;
                        let evaluation_json: Option<String> = row.get(14)?;
                        Ok(PersistedRouteReadiness {
                            install_id: row.get(0)?,
                            proposal_id: row.get(1)?,
                            route_id: row.get(2)?,
                            request_id: row.get(3)?,
                            status: row.get(4)?,
                            capital: serde_json::from_str(&capital_json).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    5,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            approvals: serde_json::from_str(&approvals_json).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    6,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            blockers: serde_json::from_str(&blockers_json).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    7,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            recommended_owner_action: recommended_owner_action_json
                                .map(|value| {
                                    serde_json::from_str(&value).map_err(|error| {
                                        rusqlite::Error::FromSqlConversionFailure(
                                            8,
                                            rusqlite::types::Type::Text,
                                            Box::new(error),
                                        )
                                    })
                                })
                                .transpose()?,
                            ordered_steps: serde_json::from_str(&ordered_steps_json).map_err(
                                |error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        9,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                },
                            )?,
                            current_step_key: row.get(10)?,
                            last_route_rejection_code: row.get(11)?,
                            last_route_rejection_message: row.get(12)?,
                            last_route_rejection_at: row.get(13)?,
                            evaluation: evaluation_json
                                .map(|value| {
                                    serde_json::from_str(&value).map_err(|error| {
                                        rusqlite::Error::FromSqlConversionFailure(
                                            14,
                                            rusqlite::types::Type::Text,
                                            Box::new(error),
                                        )
                                    })
                                })
                                .transpose()?,
                            evaluation_fingerprint: row.get(15)?,
                            stale_status: row.get(16)?,
                            stale_reason: row.get(17)?,
                            stale_detected_at: row.get(18)?,
                            evaluated_at: row.get(19)?,
                            created_at: row.get(20)?,
                            updated_at: row.get(21)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_onboarding_route_readiness_steps(
        &self,
        install_id: &str,
        proposal_id: &str,
        route_id: &str,
    ) -> Result<Vec<PersistedRouteReadinessStep>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let proposal_id = proposal_id.to_owned();
        let route_id = route_id.to_owned();

        self.connection
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT install_id, proposal_id, route_id, step_key, status, blocker_reason,
                            recommended_action_json, completed_at, created_at, updated_at
                     FROM onboarding_route_readiness_steps
                     WHERE install_id = ?1 AND proposal_id = ?2 AND route_id = ?3
                     ORDER BY rowid",
                )?;
                stmt.query_map(params![install_id, proposal_id, route_id], |row| {
                    let recommended_action_json: Option<String> = row.get(6)?;
                    Ok(PersistedRouteReadinessStep {
                        install_id: row.get(0)?,
                        proposal_id: row.get(1)?,
                        route_id: row.get(2)?,
                        step_key: row.get(3)?,
                        status: row.get(4)?,
                        blocker_reason: row.get(5)?,
                        recommended_action: recommended_action_json
                            .map(|value| {
                                serde_json::from_str(&value).map_err(|error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        6,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                })
                            })
                            .transpose()?,
                        completed_at: row.get(7)?,
                        created_at: row.get(8)?,
                        updated_at: row.get(9)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_onboarding_route_readiness_state(
        &self,
        install_id: &str,
        proposal_id: &str,
        route_id: &str,
    ) -> Result<Option<(PersistedRouteReadiness, Vec<PersistedRouteReadinessStep>)>, StateError>
    {
        let summary = self
            .load_onboarding_route_readiness(install_id, proposal_id, route_id)
            .await?;

        match summary {
            Some(summary) => {
                let steps = self
                    .load_onboarding_route_readiness_steps(install_id, proposal_id, route_id)
                    .await?;
                Ok(Some((summary, steps)))
            }
            None => Ok(None),
        }
    }

    pub async fn load_onboarding_route_readiness_states_for_proposal(
        &self,
        install_id: &str,
        proposal_id: &str,
    ) -> Result<Vec<(PersistedRouteReadiness, Vec<PersistedRouteReadinessStep>)>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let proposal_id = proposal_id.to_owned();
        let query_install_id = install_id.clone();
        let query_proposal_id = proposal_id.clone();

        let route_ids = self
            .connection
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT route_id
                     FROM onboarding_route_readiness
                     WHERE install_id = ?1 AND proposal_id = ?2
                     ORDER BY updated_at DESC, route_id",
                )?;
                stmt.query_map(params![query_install_id, query_proposal_id], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        let mut states = Vec::with_capacity(route_ids.len());
        for route_id in route_ids {
            if let Some(state) = self
                .load_onboarding_route_readiness_state(&install_id, &proposal_id, &route_id)
                .await?
            {
                states.push(state);
            }
        }
        Ok(states)
    }

    pub async fn persist_onboarding_route_readiness_state(
        &self,
        record: &PersistedRouteReadiness,
        steps: &[PersistedRouteReadinessStep],
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let record = record.clone();
        let steps = steps.to_vec();
        let capital_json =
            serde_json::to_string(&record.capital).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let approvals_json =
            serde_json::to_string(&record.approvals).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let blockers_json =
            serde_json::to_string(&record.blockers).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let recommended_owner_action_json = record
            .recommended_owner_action
            .as_ref()
            .map(|value| {
                serde_json::to_string(value).map_err(|source| StateError::Serialize {
                    path: path.clone(),
                    source,
                })
            })
            .transpose()?;
        let ordered_steps_json =
            serde_json::to_string(&record.ordered_steps).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let evaluation_json = record
            .evaluation
            .as_ref()
            .map(|value| {
                serde_json::to_string(value).map_err(|source| StateError::Serialize {
                    path: path.clone(),
                    source,
                })
            })
            .transpose()?;

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO onboarding_route_readiness (
                        install_id, proposal_id, route_id, request_id, status, capital_json,
                        approvals_json, blockers_json, recommended_owner_action_json,
                        ordered_steps_json, current_step_key,
                        last_route_rejection_code, last_route_rejection_message, last_route_rejection_at,
                        evaluation_json, evaluation_fingerprint,
                        stale_status, stale_reason, stale_detected_at,
                        evaluated_at, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
                     ON CONFLICT(install_id, proposal_id, route_id) DO UPDATE SET
                        request_id = excluded.request_id,
                        status = excluded.status,
                        capital_json = excluded.capital_json,
                        approvals_json = excluded.approvals_json,
                        blockers_json = excluded.blockers_json,
                        recommended_owner_action_json = excluded.recommended_owner_action_json,
                        ordered_steps_json = excluded.ordered_steps_json,
                        current_step_key = excluded.current_step_key,
                        last_route_rejection_code = excluded.last_route_rejection_code,
                        last_route_rejection_message = excluded.last_route_rejection_message,
                        last_route_rejection_at = excluded.last_route_rejection_at,
                        evaluation_json = excluded.evaluation_json,
                        evaluation_fingerprint = excluded.evaluation_fingerprint,
                        stale_status = excluded.stale_status,
                        stale_reason = excluded.stale_reason,
                        stale_detected_at = excluded.stale_detected_at,
                        evaluated_at = excluded.evaluated_at,
                        updated_at = excluded.updated_at",
                    params![
                        record.install_id,
                        record.proposal_id,
                        record.route_id,
                        record.request_id,
                        record.status,
                        capital_json,
                        approvals_json,
                        blockers_json,
                        recommended_owner_action_json,
                        ordered_steps_json,
                        record.current_step_key,
                        record.last_route_rejection_code,
                        record.last_route_rejection_message,
                        record.last_route_rejection_at,
                        evaluation_json,
                        record.evaluation_fingerprint,
                        record.stale_status,
                        record.stale_reason,
                        record.stale_detected_at,
                        record.evaluated_at,
                        record.created_at,
                        record.updated_at,
                    ],
                )?;

                replace_onboarding_route_readiness_steps_tx(
                    &tx,
                    &record.install_id,
                    &record.proposal_id,
                    &record.route_id,
                    &steps,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn persist_onboarding_route_readiness(
        &self,
        record: &PersistedRouteReadiness,
    ) -> Result<(), StateError> {
        self.persist_onboarding_route_readiness_state(record, &[])
            .await
    }

    pub async fn load_strategy_selection(
        &self,
        install_id: &str,
        proposal_id: &str,
    ) -> Result<Option<PersistedStrategySelection>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let proposal_id = proposal_id.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT install_id, proposal_id, selection_id, selection_revision, status,
                            reopened_from_revision, proposal_revision, proposal_uri,
                            proposal_snapshot_json, recommendation_basis_json,
                            readiness_sensitivity_summary_json, approval_json,
                            approval_stale, approval_stale_reason, created_at, updated_at
                     FROM onboarding_strategy_selections
                     WHERE install_id = ?1 AND proposal_id = ?2",
                    params![install_id, proposal_id],
                    |row| {
                        let proposal_snapshot_json: String = row.get(8)?;
                        let recommendation_basis_json: String = row.get(9)?;
                        let readiness_sensitivity_summary_json: String = row.get(10)?;
                        let approval_json: String = row.get(11)?;
                        Ok(PersistedStrategySelection {
                            install_id: row.get(0)?,
                            proposal_id: row.get(1)?,
                            selection_id: row.get(2)?,
                            selection_revision: row.get(3)?,
                            status: row.get(4)?,
                            reopened_from_revision: row.get(5)?,
                            proposal_revision: row.get(6)?,
                            proposal_uri: row.get(7)?,
                            proposal_snapshot: serde_json::from_str(&proposal_snapshot_json)
                                .map_err(|error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        8,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                })?,
                            recommendation_basis: serde_json::from_str(&recommendation_basis_json)
                                .map_err(|error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        9,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                })?,
                            readiness_sensitivity_summary: serde_json::from_str(
                                &readiness_sensitivity_summary_json,
                            )
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    10,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            approval: serde_json::from_str(&approval_json).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    11,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            approval_stale: row.get::<_, i64>(12)? != 0,
                            approval_stale_reason: row.get(13)?,
                            created_at: row.get(14)?,
                            updated_at: row.get(15)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_strategy_selection_overrides(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<Vec<PersistedStrategySelectionOverride>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let proposal_id = proposal_id.to_owned();
        let selection_id = selection_id.to_owned();

        self.connection
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT install_id, proposal_id, selection_id, selection_revision, override_key,
                            previous_value_json, new_value_json, rationale, provenance_json,
                            sensitivity_class, created_at
                     FROM onboarding_strategy_selection_overrides
                     WHERE install_id = ?1 AND proposal_id = ?2 AND selection_id = ?3
                     ORDER BY selection_revision, rowid",
                )?;
                stmt.query_map(params![install_id, proposal_id, selection_id], |row| {
                    let previous_value_json: String = row.get(5)?;
                    let new_value_json: String = row.get(6)?;
                    let provenance_json: String = row.get(8)?;
                    Ok(PersistedStrategySelectionOverride {
                        install_id: row.get(0)?,
                        proposal_id: row.get(1)?,
                        selection_id: row.get(2)?,
                        selection_revision: row.get(3)?,
                        override_key: row.get(4)?,
                        previous_value: serde_json::from_str(&previous_value_json).map_err(
                            |error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    5,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            },
                        )?,
                        new_value: serde_json::from_str(&new_value_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                6,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                        rationale: row.get(7)?,
                        provenance: serde_json::from_str(&provenance_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                8,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                        sensitivity_class: row.get(9)?,
                        created_at: row.get(10)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_strategy_selection_approval_history(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<Vec<PersistedStrategySelectionApprovalHistoryEvent>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let proposal_id = proposal_id.to_owned();
        let selection_id = selection_id.to_owned();

        self.connection
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT install_id, proposal_id, selection_id, event_kind, selection_revision,
                            approved_revision, reopened_from_revision, approved_by, note, reason,
                            provenance_json, created_at
                     FROM onboarding_strategy_selection_approval_history
                     WHERE install_id = ?1 AND proposal_id = ?2 AND selection_id = ?3
                     ORDER BY history_id",
                )?;
                stmt.query_map(params![install_id, proposal_id, selection_id], |row| {
                    let provenance_json: Option<String> = row.get(10)?;
                    Ok(PersistedStrategySelectionApprovalHistoryEvent {
                        install_id: row.get(0)?,
                        proposal_id: row.get(1)?,
                        selection_id: row.get(2)?,
                        event_kind: row.get(3)?,
                        selection_revision: row.get(4)?,
                        approved_revision: row.get(5)?,
                        reopened_from_revision: row.get(6)?,
                        approved_by: row.get(7)?,
                        note: row.get(8)?,
                        reason: row.get(9)?,
                        provenance: provenance_json
                            .map(|value| {
                                serde_json::from_str(&value).map_err(|error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        10,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                })
                            })
                            .transpose()?,
                        created_at: row.get(11)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn persist_strategy_selection_materialized(
        &self,
        selection: &PersistedStrategySelection,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let selection = selection.clone();
        let proposal_snapshot_json =
            serde_json::to_string(&selection.proposal_snapshot).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let recommendation_basis_json = serde_json::to_string(&selection.recommendation_basis)
            .map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let readiness_sensitivity_summary_json =
            serde_json::to_string(&selection.readiness_sensitivity_summary).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let approval_json =
            serde_json::to_string(&selection.approval).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let journal_payload = serde_json::json!({
            "install_id": selection.install_id,
            "proposal_id": selection.proposal_id,
            "selection_id": selection.selection_id,
            "proposal_revision": selection.proposal_revision,
            "selection_revision": selection.selection_revision,
            "status": selection.status,
        });

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO onboarding_strategy_selections (
                        install_id, proposal_id, selection_id, selection_revision, status,
                        reopened_from_revision, proposal_revision, proposal_uri,
                        proposal_snapshot_json, recommendation_basis_json,
                        readiness_sensitivity_summary_json, approval_json,
                        approval_stale, approval_stale_reason, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                     ON CONFLICT(install_id, proposal_id) DO UPDATE SET
                        selection_id = excluded.selection_id,
                        selection_revision = excluded.selection_revision,
                        status = excluded.status,
                        reopened_from_revision = excluded.reopened_from_revision,
                        proposal_revision = excluded.proposal_revision,
                        proposal_uri = excluded.proposal_uri,
                        proposal_snapshot_json = excluded.proposal_snapshot_json,
                        recommendation_basis_json = excluded.recommendation_basis_json,
                        readiness_sensitivity_summary_json = excluded.readiness_sensitivity_summary_json,
                        approval_json = excluded.approval_json,
                        approval_stale = excluded.approval_stale,
                        approval_stale_reason = excluded.approval_stale_reason,
                        updated_at = excluded.updated_at",
                    params![
                        selection.install_id,
                        selection.proposal_id,
                        selection.selection_id,
                        selection.selection_revision,
                        selection.status,
                        selection.reopened_from_revision,
                        selection.proposal_revision,
                        selection.proposal_uri,
                        proposal_snapshot_json,
                        recommendation_basis_json,
                        readiness_sensitivity_summary_json,
                        approval_json,
                        if selection.approval_stale { 1 } else { 0 },
                        selection.approval_stale_reason,
                        selection.created_at,
                        selection.updated_at,
                    ],
                )?;
                append_journal_entry(
                    &tx,
                    "strategy_selection",
                    &selection.proposal_id,
                    STRATEGY_SELECTION_MATERIALIZED_EVENT_TYPE,
                    journal_payload.to_string(),
                    &selection.updated_at,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn persist_strategy_selection_override(
        &self,
        selection: &PersistedStrategySelection,
        override_record: &PersistedStrategySelectionOverride,
        approval_history_event: Option<&PersistedStrategySelectionApprovalHistoryEvent>,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let selection = selection.clone();
        let override_record = override_record.clone();
        let proposal_snapshot_json =
            serde_json::to_string(&selection.proposal_snapshot).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let recommendation_basis_json = serde_json::to_string(&selection.recommendation_basis)
            .map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let readiness_sensitivity_summary_json =
            serde_json::to_string(&selection.readiness_sensitivity_summary).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let approval_json =
            serde_json::to_string(&selection.approval).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let previous_value_json =
            serde_json::to_string(&override_record.previous_value).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let new_value_json =
            serde_json::to_string(&override_record.new_value).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let provenance_json =
            serde_json::to_string(&override_record.provenance).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let approval_history_event = approval_history_event.cloned();
        let approval_history_provenance_json = approval_history_event
            .as_ref()
            .and_then(|event| event.provenance.as_ref())
            .map(|value| {
                serde_json::to_string(value).map_err(|source| StateError::Serialize {
                    path: path.clone(),
                    source,
                })
            })
            .transpose()?;
        let journal_payload = serde_json::json!({
            "install_id": selection.install_id,
            "proposal_id": selection.proposal_id,
            "selection_id": selection.selection_id,
            "selection_revision": selection.selection_revision,
            "override_key": override_record.override_key,
            "sensitivity_class": override_record.sensitivity_class,
        });
        let reopened_payload = approval_history_event.as_ref().map(|event| {
            serde_json::json!({
                "install_id": event.install_id,
                "proposal_id": event.proposal_id,
                "selection_id": event.selection_id,
                "selection_revision": event.selection_revision,
                "reopened_from_revision": event.reopened_from_revision,
                "reason": event.reason,
            })
            .to_string()
        });

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "UPDATE onboarding_strategy_selections
                     SET selection_revision = ?4,
                         status = ?5,
                         reopened_from_revision = ?6,
                         proposal_revision = ?7,
                         proposal_uri = ?8,
                         proposal_snapshot_json = ?9,
                         recommendation_basis_json = ?10,
                         readiness_sensitivity_summary_json = ?11,
                         approval_json = ?12,
                         approval_stale = ?13,
                         approval_stale_reason = ?14,
                         updated_at = ?15
                     WHERE install_id = ?1 AND proposal_id = ?2 AND selection_id = ?3",
                    params![
                        selection.install_id,
                        selection.proposal_id,
                        selection.selection_id,
                        selection.selection_revision,
                        selection.status,
                        selection.reopened_from_revision,
                        selection.proposal_revision,
                        selection.proposal_uri,
                        proposal_snapshot_json,
                        recommendation_basis_json,
                        readiness_sensitivity_summary_json,
                        approval_json,
                        if selection.approval_stale { 1 } else { 0 },
                        selection.approval_stale_reason,
                        selection.updated_at,
                    ],
                )?;
                tx.execute(
                    "INSERT INTO onboarding_strategy_selection_overrides (
                        install_id, proposal_id, selection_id, selection_revision, override_key,
                        previous_value_json, new_value_json, rationale, provenance_json,
                        sensitivity_class, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        override_record.install_id,
                        override_record.proposal_id,
                        override_record.selection_id,
                        override_record.selection_revision,
                        override_record.override_key,
                        previous_value_json,
                        new_value_json,
                        override_record.rationale,
                        provenance_json,
                        override_record.sensitivity_class,
                        override_record.created_at,
                    ],
                )?;
                if let Some(event) = approval_history_event.as_ref() {
                    tx.execute(
                        "INSERT INTO onboarding_strategy_selection_approval_history (
                            install_id, proposal_id, selection_id, event_kind, selection_revision,
                            approved_revision, reopened_from_revision, approved_by, note, reason,
                            provenance_json, created_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                        params![
                            event.install_id,
                            event.proposal_id,
                            event.selection_id,
                            event.event_kind,
                            event.selection_revision,
                            event.approved_revision,
                            event.reopened_from_revision,
                            event.approved_by,
                            event.note,
                            event.reason,
                            approval_history_provenance_json,
                            event.created_at,
                        ],
                    )?;
                }
                append_journal_entry(
                    &tx,
                    "strategy_selection",
                    &selection.selection_id,
                    STRATEGY_SELECTION_OVERRIDE_APPLIED_EVENT_TYPE,
                    journal_payload.to_string(),
                    &selection.updated_at,
                )?;
                if let Some(payload) = reopened_payload {
                    append_journal_entry(
                        &tx,
                        "strategy_selection",
                        &selection.selection_id,
                        STRATEGY_SELECTION_REOPENED_EVENT_TYPE,
                        payload,
                        &selection.updated_at,
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn persist_strategy_selection_approval(
        &self,
        selection: &PersistedStrategySelection,
        approval_history_event: &PersistedStrategySelectionApprovalHistoryEvent,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let selection = selection.clone();
        let proposal_snapshot_json =
            serde_json::to_string(&selection.proposal_snapshot).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let recommendation_basis_json = serde_json::to_string(&selection.recommendation_basis)
            .map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let readiness_sensitivity_summary_json =
            serde_json::to_string(&selection.readiness_sensitivity_summary).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let approval_json =
            serde_json::to_string(&selection.approval).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let approval_history_event = approval_history_event.clone();
        let provenance_json = approval_history_event
            .provenance
            .as_ref()
            .map(|value| {
                serde_json::to_string(value).map_err(|source| StateError::Serialize {
                    path: path.clone(),
                    source,
                })
            })
            .transpose()?;
        let journal_payload = serde_json::json!({
            "install_id": selection.install_id,
            "proposal_id": selection.proposal_id,
            "selection_id": selection.selection_id,
            "selection_revision": selection.selection_revision,
            "status": selection.status,
            "approval": selection.approval,
        });

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "UPDATE onboarding_strategy_selections
                     SET selection_revision = ?4,
                         status = ?5,
                         reopened_from_revision = ?6,
                         proposal_revision = ?7,
                         proposal_uri = ?8,
                         proposal_snapshot_json = ?9,
                         recommendation_basis_json = ?10,
                         readiness_sensitivity_summary_json = ?11,
                         approval_json = ?12,
                         approval_stale = ?13,
                         approval_stale_reason = ?14,
                         updated_at = ?15
                     WHERE install_id = ?1 AND proposal_id = ?2 AND selection_id = ?3",
                    params![
                        selection.install_id,
                        selection.proposal_id,
                        selection.selection_id,
                        selection.selection_revision,
                        selection.status,
                        selection.reopened_from_revision,
                        selection.proposal_revision,
                        selection.proposal_uri,
                        proposal_snapshot_json,
                        recommendation_basis_json,
                        readiness_sensitivity_summary_json,
                        approval_json,
                        if selection.approval_stale { 1 } else { 0 },
                        selection.approval_stale_reason,
                        selection.updated_at,
                    ],
                )?;
                tx.execute(
                    "INSERT INTO onboarding_strategy_selection_approval_history (
                        install_id, proposal_id, selection_id, event_kind, selection_revision,
                        approved_revision, reopened_from_revision, approved_by, note, reason,
                        provenance_json, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        approval_history_event.install_id,
                        approval_history_event.proposal_id,
                        approval_history_event.selection_id,
                        approval_history_event.event_kind,
                        approval_history_event.selection_revision,
                        approval_history_event.approved_revision,
                        approval_history_event.reopened_from_revision,
                        approval_history_event.approved_by,
                        approval_history_event.note,
                        approval_history_event.reason,
                        provenance_json,
                        approval_history_event.created_at,
                    ],
                )?;
                append_journal_entry(
                    &tx,
                    "strategy_selection",
                    &selection.selection_id,
                    STRATEGY_SELECTION_APPROVED_EVENT_TYPE,
                    journal_payload.to_string(),
                    &selection.updated_at,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_strategy_runtime_handoff(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<Option<PersistedStrategyRuntimeHandoff>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();
        let proposal_id = proposal_id.to_owned();
        let selection_id = selection_id.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT install_id, proposal_id, selection_id, approved_selection_revision,
                            route_id, request_id, route_readiness_fingerprint,
                            route_readiness_status, route_readiness_evaluated_at,
                            eligibility_status, hold_reason, runtime_control_mode,
                            strategy_id, runtime_identity_refreshed_at,
                            runtime_identity_source, created_at, updated_at
                     FROM onboarding_strategy_runtime_handoffs
                     WHERE install_id = ?1 AND proposal_id = ?2 AND selection_id = ?3",
                    params![install_id, proposal_id, selection_id],
                    |row| {
                        Ok(PersistedStrategyRuntimeHandoff {
                            install_id: row.get(0)?,
                            proposal_id: row.get(1)?,
                            selection_id: row.get(2)?,
                            approved_selection_revision: row.get(3)?,
                            route_id: row.get(4)?,
                            request_id: row.get(5)?,
                            route_readiness_fingerprint: row.get(6)?,
                            route_readiness_status: row.get(7)?,
                            route_readiness_evaluated_at: row.get(8)?,
                            eligibility_status: row.get(9)?,
                            hold_reason: row.get(10)?,
                            runtime_control_mode: row.get(11)?,
                            strategy_id: row.get(12)?,
                            runtime_identity_refreshed_at: row.get(13)?,
                            runtime_identity_source: row.get(14)?,
                            created_at: row.get(15)?,
                            updated_at: row.get(16)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_latest_strategy_runtime_handoff_for_install(
        &self,
        install_id: &str,
    ) -> Result<Option<PersistedStrategyRuntimeHandoff>, StateError> {
        let path = self.path.clone();
        let install_id = install_id.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT install_id, proposal_id, selection_id, approved_selection_revision,
                            route_id, request_id, route_readiness_fingerprint,
                            route_readiness_status, route_readiness_evaluated_at,
                            eligibility_status, hold_reason, runtime_control_mode,
                            strategy_id, runtime_identity_refreshed_at,
                            runtime_identity_source, created_at, updated_at
                     FROM onboarding_strategy_runtime_handoffs
                     WHERE install_id = ?1
                     ORDER BY updated_at DESC, proposal_id, selection_id
                     LIMIT 1",
                    [install_id],
                    |row| {
                        Ok(PersistedStrategyRuntimeHandoff {
                            install_id: row.get(0)?,
                            proposal_id: row.get(1)?,
                            selection_id: row.get(2)?,
                            approved_selection_revision: row.get(3)?,
                            route_id: row.get(4)?,
                            request_id: row.get(5)?,
                            route_readiness_fingerprint: row.get(6)?,
                            route_readiness_status: row.get(7)?,
                            route_readiness_evaluated_at: row.get(8)?,
                            eligibility_status: row.get(9)?,
                            hold_reason: row.get(10)?,
                            runtime_control_mode: row.get(11)?,
                            strategy_id: row.get(12)?,
                            runtime_identity_refreshed_at: row.get(13)?,
                            runtime_identity_source: row.get(14)?,
                            created_at: row.get(15)?,
                            updated_at: row.get(16)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn persist_strategy_runtime_handoff(
        &self,
        handoff: &PersistedStrategyRuntimeHandoff,
        append_handoff_event: bool,
        append_eligibility_event: bool,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let handoff = handoff.clone();
        let handoff_payload = serde_json::json!({
            "install_id": &handoff.install_id,
            "proposal_id": &handoff.proposal_id,
            "selection_id": &handoff.selection_id,
            "approved_selection_revision": handoff.approved_selection_revision,
            "route_id": &handoff.route_id,
            "request_id": &handoff.request_id,
            "route_readiness_fingerprint": &handoff.route_readiness_fingerprint,
            "route_readiness_status": &handoff.route_readiness_status,
            "route_readiness_evaluated_at": &handoff.route_readiness_evaluated_at,
            "eligibility_status": &handoff.eligibility_status,
            "hold_reason": &handoff.hold_reason,
            "runtime_control_mode": &handoff.runtime_control_mode,
            "strategy_id": &handoff.strategy_id,
            "runtime_identity_refreshed_at": &handoff.runtime_identity_refreshed_at,
            "runtime_identity_source": &handoff.runtime_identity_source,
        })
        .to_string();
        let eligibility_payload = serde_json::json!({
            "install_id": &handoff.install_id,
            "proposal_id": &handoff.proposal_id,
            "selection_id": &handoff.selection_id,
            "eligibility_status": &handoff.eligibility_status,
            "hold_reason": &handoff.hold_reason,
            "runtime_control_mode": &handoff.runtime_control_mode,
            "approved_selection_revision": handoff.approved_selection_revision,
            "route_id": &handoff.route_id,
            "request_id": &handoff.request_id,
        })
        .to_string();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO onboarding_strategy_runtime_handoffs (
                        install_id, proposal_id, selection_id, approved_selection_revision,
                        route_id, request_id, route_readiness_fingerprint,
                        route_readiness_status, route_readiness_evaluated_at,
                        eligibility_status, hold_reason, runtime_control_mode,
                        strategy_id, runtime_identity_refreshed_at, runtime_identity_source,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                     ON CONFLICT(install_id, proposal_id, selection_id) DO UPDATE SET
                        approved_selection_revision = excluded.approved_selection_revision,
                        route_id = excluded.route_id,
                        request_id = excluded.request_id,
                        route_readiness_fingerprint = excluded.route_readiness_fingerprint,
                        route_readiness_status = excluded.route_readiness_status,
                        route_readiness_evaluated_at = excluded.route_readiness_evaluated_at,
                        eligibility_status = excluded.eligibility_status,
                        hold_reason = excluded.hold_reason,
                        runtime_control_mode = excluded.runtime_control_mode,
                        strategy_id = excluded.strategy_id,
                        runtime_identity_refreshed_at = excluded.runtime_identity_refreshed_at,
                        runtime_identity_source = excluded.runtime_identity_source,
                        updated_at = excluded.updated_at",
                    params![
                        handoff.install_id,
                        handoff.proposal_id,
                        handoff.selection_id,
                        handoff.approved_selection_revision,
                        handoff.route_id,
                        handoff.request_id,
                        handoff.route_readiness_fingerprint,
                        handoff.route_readiness_status,
                        handoff.route_readiness_evaluated_at,
                        handoff.eligibility_status,
                        handoff.hold_reason,
                        handoff.runtime_control_mode,
                        handoff.strategy_id,
                        handoff.runtime_identity_refreshed_at,
                        handoff.runtime_identity_source,
                        handoff.created_at,
                        handoff.updated_at,
                    ],
                )?;
                if append_handoff_event {
                    append_journal_entry(
                        &tx,
                        STRATEGY_RUNTIME_STREAM_TYPE,
                        &handoff.selection_id,
                        STRATEGY_RUNTIME_HANDOFF_EVENT_TYPE,
                        handoff_payload.clone(),
                        &handoff.updated_at,
                    )?;
                }
                if append_eligibility_event {
                    append_journal_entry(
                        &tx,
                        STRATEGY_RUNTIME_STREAM_TYPE,
                        &handoff.selection_id,
                        STRATEGY_RUNTIME_ELIGIBILITY_EVENT_TYPE,
                        eligibility_payload,
                        &handoff.updated_at,
                    )?;
                }
                if handoff.strategy_id.is_some() && handoff.runtime_identity_refreshed_at.is_some() {
                    append_journal_entry(
                        &tx,
                        STRATEGY_RUNTIME_STREAM_TYPE,
                        &handoff.selection_id,
                        STRATEGY_RUNTIME_IDENTITY_REFRESHED_EVENT_TYPE,
                        handoff_payload.clone(),
                        handoff.runtime_identity_refreshed_at.as_deref().unwrap_or(&handoff.updated_at),
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn persist_snapshot(
        &self,
        snapshot: &CanonicalStateSnapshot,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let snapshot = snapshot.clone();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;

                for strategy in &snapshot.strategies {
                    tx.execute(
                        "INSERT INTO strategy_runtime_states (strategy_id, runtime_state, last_transition_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(strategy_id) DO UPDATE SET
                            runtime_state = excluded.runtime_state,
                            last_transition_at = excluded.last_transition_at,
                            updated_at = excluded.updated_at",
                        params![
                            strategy.strategy_id,
                            strategy.runtime_state,
                            strategy.last_transition_at,
                            strategy.updated_at,
                        ],
                    )?;
                    append_journal_entry(
                        &tx,
                        "strategy",
                        &strategy.strategy_id,
                        STRATEGY_EVENT_TYPE,
                        format!(
                            "{{\"strategy_id\":\"{}\",\"runtime_state\":\"{}\",\"last_transition_at\":\"{}\",\"updated_at\":\"{}\"}}",
                            strategy.strategy_id, strategy.runtime_state, strategy.last_transition_at, strategy.updated_at,
                        ),
                        &strategy.updated_at,
                    )?;
                }

                for execution in &snapshot.executions {
                    tx.execute(
                        "INSERT INTO execution_states (execution_id, plan_id, status, updated_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(execution_id) DO UPDATE SET
                            plan_id = excluded.plan_id,
                            status = excluded.status,
                            updated_at = excluded.updated_at",
                        params![
                            execution.execution_id,
                            execution.plan_id,
                            execution.status,
                            execution.updated_at,
                        ],
                    )?;
                    append_journal_entry(
                        &tx,
                        "execution",
                        &execution.execution_id,
                        EXECUTION_EVENT_TYPE,
                        format!(
                            "{{\"execution_id\":\"{}\",\"plan_id\":\"{}\",\"status\":\"{}\",\"updated_at\":\"{}\"}}",
                            execution.execution_id, execution.plan_id, execution.status, execution.updated_at,
                        ),
                        &execution.updated_at,
                    )?;
                }

                for reconciliation in &snapshot.reconciliations {
                    tx.execute(
                        "INSERT INTO reconciliation_states (execution_id, residual_exposure_usd, rebalance_required, updated_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(execution_id) DO UPDATE SET
                            residual_exposure_usd = excluded.residual_exposure_usd,
                            rebalance_required = excluded.rebalance_required,
                            updated_at = excluded.updated_at",
                        params![
                            reconciliation.execution_id,
                            reconciliation.residual_exposure_usd,
                            if reconciliation.rebalance_required { 1 } else { 0 },
                            reconciliation.updated_at,
                        ],
                    )?;
                    append_journal_entry(
                        &tx,
                        "reconciliation",
                        &reconciliation.execution_id,
                        RECONCILIATION_EVENT_TYPE,
                        format!(
                            "{{\"execution_id\":\"{}\",\"residual_exposure_usd\":{},\"rebalance_required\":{},\"updated_at\":\"{}\"}}",
                            reconciliation.execution_id,
                            reconciliation.residual_exposure_usd,
                            if reconciliation.rebalance_required { "true" } else { "false" },
                            reconciliation.updated_at,
                        ),
                        &reconciliation.updated_at,
                    )?;
                }

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn persist_execution_state(
        &self,
        execution_id: &str,
        plan_id: &str,
        status: &str,
    ) -> Result<ExecutionStateRecord, StateError> {
        self.persist_execution_state_with_metadata(execution_id, plan_id, status, None)
            .await
    }

    pub async fn persist_execution_state_with_metadata(
        &self,
        execution_id: &str,
        plan_id: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<ExecutionStateRecord, StateError> {
        let path = self.path.clone();
        let execution_id = execution_id.to_owned();
        let plan_id = plan_id.to_owned();
        let status = status.to_owned();
        let metadata = metadata.clone();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO execution_states (execution_id, plan_id, status, updated_at)
                     VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
                     ON CONFLICT(execution_id) DO UPDATE SET
                        plan_id = excluded.plan_id,
                        status = excluded.status,
                        updated_at = CURRENT_TIMESTAMP",
                    params![execution_id, plan_id, status],
                )?;

                let record = tx.query_row(
                    "SELECT execution_id, plan_id, status, updated_at
                     FROM execution_states
                     WHERE execution_id = ?1",
                    [execution_id.as_str()],
                    |row| {
                        Ok(ExecutionStateRecord {
                            execution_id: row.get(0)?,
                            plan_id: row.get(1)?,
                            status: row.get(2)?,
                            updated_at: row.get(3)?,
                        })
                    },
                )?;

                let payload_json = serde_json::to_string(&ExecutionLifecyclePayload {
                    execution_id: record.execution_id.clone(),
                    plan_id: record.plan_id.clone(),
                    status: record.status.clone(),
                    updated_at: record.updated_at.clone(),
                    metadata: metadata.clone(),
                })
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;

                append_journal_entry(
                    &tx,
                    "execution",
                    &record.execution_id,
                    EXECUTION_EVENT_TYPE,
                    payload_json,
                    &record.updated_at,
                )?;

                tx.commit()?;
                Ok(record)
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn persist_reconciliation_state(
        &self,
        execution_id: &str,
        residual_exposure_usd: i64,
        rebalance_required: bool,
        updated_at: &str,
    ) -> Result<ReconciliationStateRecord, StateError> {
        let path = self.path.clone();
        let execution_id = execution_id.to_owned();
        let updated_at = updated_at.to_owned();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO reconciliation_states (execution_id, residual_exposure_usd, rebalance_required, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(execution_id) DO UPDATE SET
                        residual_exposure_usd = excluded.residual_exposure_usd,
                        rebalance_required = excluded.rebalance_required,
                        updated_at = excluded.updated_at",
                    params![
                        execution_id,
                        residual_exposure_usd,
                        if rebalance_required { 1 } else { 0 },
                        updated_at,
                    ],
                )?;

                let record = tx.query_row(
                    "SELECT execution_id, residual_exposure_usd, rebalance_required, updated_at
                     FROM reconciliation_states
                     WHERE execution_id = ?1",
                    [execution_id.as_str()],
                    |row| {
                        Ok(ReconciliationStateRecord {
                            execution_id: row.get(0)?,
                            residual_exposure_usd: row.get(1)?,
                            rebalance_required: row.get::<_, i64>(2)? != 0,
                            updated_at: row.get(3)?,
                        })
                    },
                )?;

                append_journal_entry(
                    &tx,
                    "reconciliation",
                    &record.execution_id,
                    RECONCILIATION_EVENT_TYPE,
                    serde_json::to_string(&record).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?,
                    &record.updated_at,
                )?;

                tx.commit()?;
                Ok(record)
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_intent_envelope(
        &self,
        request_id: &str,
    ) -> Result<Option<AgentRequestEnvelope<Intent>>, StateError> {
        let path = self.path.clone();
        let request_id = request_id.to_owned();

        let raw = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT request_id, source_agent_id, submitted_at, payload_json, rationale_json, execution_prefs_json
                     FROM agent_requests
                     WHERE request_id = ?1 AND request_kind = 'intent'",
                    [request_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path: path.clone(), source })?;

        raw.map(
            |(
                request_id,
                source_agent_id,
                submitted_at,
                payload_json,
                rationale_json,
                execution_prefs_json,
            )| {
                Ok(AgentRequestEnvelope {
                    request_id,
                    request_kind: a2ex_control::AgentRequestKind::Intent,
                    source_agent_id,
                    submitted_at,
                    payload: serde_json::from_str(&payload_json).map_err(|source| {
                        StateError::Deserialize {
                            path: path.clone(),
                            source,
                        }
                    })?,
                    rationale: serde_json::from_str(&rationale_json).map_err(|source| {
                        StateError::Deserialize {
                            path: path.clone(),
                            source,
                        }
                    })?,
                    execution_preferences: serde_json::from_str(&execution_prefs_json).map_err(
                        |source| StateError::Deserialize {
                            path: path.clone(),
                            source,
                        },
                    )?,
                })
            },
        )
        .transpose()
    }

    pub async fn persist_intent_submission(
        &self,
        envelope: &AgentRequestEnvelope<Intent>,
    ) -> Result<PersistedIntentSubmission, StateError> {
        let path = self.path.clone();
        let request_id = envelope.request_id.clone();
        let request_kind = format!("{:?}", envelope.request_kind).to_lowercase();
        let source_agent_id = envelope.source_agent_id.clone();
        let submitted_at = envelope.submitted_at.clone();
        let updated_at = envelope.submitted_at.clone();
        let intent_id = envelope.payload.intent_id.clone();
        let intent_type = envelope.payload.intent_type.clone();
        let payload_json =
            serde_json::to_string(&envelope.payload).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let objective_json =
            serde_json::to_string(&envelope.payload.objective).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let constraints_json =
            serde_json::to_string(&envelope.payload.constraints).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let funding_json = serde_json::to_string(&envelope.payload.funding).map_err(|source| {
            StateError::Serialize {
                path: path.clone(),
                source,
            }
        })?;
        let post_actions_json =
            serde_json::to_string(&envelope.payload.post_actions).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let rationale_json =
            serde_json::to_string(&envelope.rationale).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let execution_prefs_json =
            serde_json::to_string(&envelope.execution_preferences).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let envelope_json =
            serde_json::to_string(envelope).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO agent_requests (
                        request_id, request_kind, source_agent_id, submitted_at, payload_json, rationale_json, execution_prefs_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(request_id) DO UPDATE SET
                        request_kind = excluded.request_kind,
                        source_agent_id = excluded.source_agent_id,
                        submitted_at = excluded.submitted_at,
                        payload_json = excluded.payload_json,
                        rationale_json = excluded.rationale_json,
                        execution_prefs_json = excluded.execution_prefs_json",
                    params![
                        request_id,
                        request_kind,
                        source_agent_id,
                        submitted_at,
                        payload_json,
                        rationale_json,
                        execution_prefs_json,
                    ],
                )?;

                tx.execute(
                    "INSERT INTO intents (
                        intent_id, request_id, source_agent_id, intent_type, objective_json, constraints_json, funding_json,
                        post_actions_json, rationale_json, execution_prefs_json, submitted_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT(intent_id) DO UPDATE SET
                        request_id = excluded.request_id,
                        source_agent_id = excluded.source_agent_id,
                        intent_type = excluded.intent_type,
                        objective_json = excluded.objective_json,
                        constraints_json = excluded.constraints_json,
                        funding_json = excluded.funding_json,
                        post_actions_json = excluded.post_actions_json,
                        rationale_json = excluded.rationale_json,
                        execution_prefs_json = excluded.execution_prefs_json,
                        submitted_at = excluded.submitted_at,
                        updated_at = excluded.updated_at",
                    params![
                        intent_id,
                        request_id,
                        source_agent_id,
                        intent_type,
                        objective_json,
                        constraints_json,
                        funding_json,
                        post_actions_json,
                        rationale_json,
                        execution_prefs_json,
                        submitted_at,
                        updated_at,
                    ],
                )?;

                append_journal_entry(
                    &tx,
                    "intent",
                    &intent_id,
                    INTENT_EVENT_TYPE,
                    envelope_json.clone(),
                    &submitted_at,
                )?;
                append_journal_entry(
                    &tx,
                    "agent_request",
                    &request_id,
                    INTENT_EVENT_TYPE,
                    envelope_json,
                    &submitted_at,
                )?;

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path: path.clone(), source })?;

        Ok(PersistedIntentSubmission {
            request_id: envelope.request_id.clone(),
            intent_id: envelope.payload.intent_id.clone(),
            source_agent_id: envelope.source_agent_id.clone(),
            intent_type: envelope.payload.intent_type.clone(),
            rationale: envelope.rationale.clone(),
            execution_preferences: envelope.execution_preferences.clone(),
            submitted_at: envelope.submitted_at.clone(),
            updated_at: envelope.submitted_at.clone(),
        })
    }

    pub async fn persist_strategy_registration(
        &self,
        envelope: &AgentRequestEnvelope<Strategy>,
    ) -> Result<PersistedStrategyRegistration, StateError> {
        let path = self.path.clone();
        let request_id = envelope.request_id.clone();
        let request_kind = format!("{:?}", envelope.request_kind).to_lowercase();
        let source_agent_id = envelope.source_agent_id.clone();
        let submitted_at = envelope.submitted_at.clone();
        let strategy_id = envelope.payload.strategy_id.clone();
        let payload_json =
            serde_json::to_string(&envelope.payload).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let rationale_json =
            serde_json::to_string(&envelope.rationale).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;
        let execution_prefs_json =
            serde_json::to_string(&envelope.execution_preferences).map_err(|source| {
                StateError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        let envelope_json =
            serde_json::to_string(envelope).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO agent_requests (
                        request_id, request_kind, source_agent_id, submitted_at, payload_json, rationale_json, execution_prefs_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(request_id) DO UPDATE SET
                        request_kind = excluded.request_kind,
                        source_agent_id = excluded.source_agent_id,
                        submitted_at = excluded.submitted_at,
                        payload_json = excluded.payload_json,
                        rationale_json = excluded.rationale_json,
                        execution_prefs_json = excluded.execution_prefs_json",
                    params![
                        request_id,
                        request_kind,
                        source_agent_id,
                        submitted_at,
                        payload_json,
                        rationale_json,
                        execution_prefs_json,
                    ],
                )?;

                tx.execute(
                    "INSERT INTO strategy_runtime_states (strategy_id, runtime_state, last_transition_at, updated_at)
                     VALUES (?1, 'idle', ?2, ?2)
                     ON CONFLICT(strategy_id) DO UPDATE SET
                        runtime_state = excluded.runtime_state,
                        last_transition_at = excluded.last_transition_at,
                        updated_at = excluded.updated_at",
                    params![strategy_id, submitted_at],
                )?;

                tx.execute(
                    "INSERT INTO strategy_runtime_recovery (strategy_id, runtime_state, next_tick_at, last_event_id, metrics_json, updated_at)
                     VALUES (?1, 'idle', NULL, NULL, '{}', ?2)
                     ON CONFLICT(strategy_id) DO UPDATE SET
                        runtime_state = excluded.runtime_state,
                        next_tick_at = excluded.next_tick_at,
                        last_event_id = excluded.last_event_id,
                        metrics_json = excluded.metrics_json,
                        updated_at = excluded.updated_at",
                    params![strategy_id, submitted_at],
                )?;

                append_journal_entry(
                    &tx,
                    "strategy",
                    &strategy_id,
                    STRATEGY_REGISTRATION_EVENT_TYPE,
                    envelope_json.clone(),
                    &submitted_at,
                )?;
                append_journal_entry(
                    &tx,
                    "agent_request",
                    &request_id,
                    STRATEGY_REGISTRATION_EVENT_TYPE,
                    envelope_json,
                    &submitted_at,
                )?;

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path: path.clone(), source })?;

        Ok(PersistedStrategyRegistration {
            request_id: envelope.request_id.clone(),
            strategy_id: envelope.payload.strategy_id.clone(),
            source_agent_id: envelope.source_agent_id.clone(),
            strategy_type: envelope.payload.strategy_type.clone(),
            watchers: envelope.payload.watchers.clone(),
            trigger_rules: envelope.payload.trigger_rules.clone(),
            calculation_model: envelope.payload.calculation_model.clone(),
            action_templates: envelope.payload.action_templates.clone(),
            constraints: envelope.payload.constraints.clone(),
            unwind_rules: envelope.payload.unwind_rules.clone(),
            rationale: envelope.rationale.clone(),
            execution_preferences: envelope.execution_preferences.clone(),
            submitted_at: envelope.submitted_at.clone(),
            updated_at: envelope.submitted_at.clone(),
        })
    }

    pub async fn load_intent_submission(
        &self,
        intent_id: &str,
    ) -> Result<Option<PersistedIntentSubmission>, StateError> {
        let path = self.path.clone();
        let intent_id = intent_id.to_owned();

        let raw = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT request_id, intent_id, source_agent_id, intent_type, rationale_json,
                            execution_prefs_json, submitted_at, updated_at
                     FROM intents
                     WHERE intent_id = ?1",
                    [intent_id],
                    |row| {
                        Ok(RawPersistedIntentSubmission {
                            request_id: row.get(0)?,
                            intent_id: row.get(1)?,
                            source_agent_id: row.get(2)?,
                            intent_type: row.get(3)?,
                            rationale_json: row.get(4)?,
                            execution_prefs_json: row.get(5)?,
                            submitted_at: row.get(6)?,
                            updated_at: row.get(7)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        raw.map(|record| {
            Ok(PersistedIntentSubmission {
                request_id: record.request_id,
                intent_id: record.intent_id,
                source_agent_id: record.source_agent_id,
                intent_type: record.intent_type,
                rationale: serde_json::from_str(&record.rationale_json).map_err(|source| {
                    StateError::Deserialize {
                        path: path.clone(),
                        source,
                    }
                })?,
                execution_preferences: serde_json::from_str(&record.execution_prefs_json).map_err(
                    |source| StateError::Deserialize {
                        path: path.clone(),
                        source,
                    },
                )?,
                submitted_at: record.submitted_at,
                updated_at: record.updated_at,
            })
        })
        .transpose()
    }

    pub async fn load_strategy_registration(
        &self,
        strategy_id: &str,
    ) -> Result<Option<PersistedStrategyRegistration>, StateError> {
        let path = self.path.clone();
        let strategy_id = strategy_id.to_owned();

        let raw = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT ar.request_id, srs.strategy_id, ar.source_agent_id, ar.payload_json,
                            ar.rationale_json, ar.execution_prefs_json, ar.submitted_at
                     FROM strategy_runtime_states srs
                     INNER JOIN agent_requests ar ON json_extract(ar.payload_json, '$.strategy_id') = srs.strategy_id
                     WHERE srs.strategy_id = ?1 AND ar.request_kind = 'strategy'
                     ORDER BY ar.submitted_at DESC
                     LIMIT 1",
                    [strategy_id],
                    |row| {
                        Ok(RawPersistedStrategyRegistration {
                            request_id: row.get(0)?,
                            strategy_id: row.get(1)?,
                            source_agent_id: row.get(2)?,
                            payload_json: row.get(3)?,
                            rationale_json: row.get(4)?,
                            execution_prefs_json: row.get(5)?,
                            submitted_at: row.get(6)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path: path.clone(), source })?;

        raw.map(|record| {
            let payload: Strategy =
                serde_json::from_str(&record.payload_json).map_err(|source| {
                    StateError::Deserialize {
                        path: path.clone(),
                        source,
                    }
                })?;

            Ok(PersistedStrategyRegistration {
                request_id: record.request_id,
                strategy_id: record.strategy_id,
                source_agent_id: record.source_agent_id,
                strategy_type: payload.strategy_type,
                watchers: payload.watchers,
                trigger_rules: payload.trigger_rules,
                calculation_model: payload.calculation_model,
                action_templates: payload.action_templates,
                constraints: payload.constraints,
                unwind_rules: payload.unwind_rules,
                rationale: serde_json::from_str(&record.rationale_json).map_err(|source| {
                    StateError::Deserialize {
                        path: path.clone(),
                        source,
                    }
                })?,
                execution_preferences: serde_json::from_str(&record.execution_prefs_json).map_err(
                    |source| StateError::Deserialize {
                        path: path.clone(),
                        source,
                    },
                )?,
                submitted_at: record.submitted_at.clone(),
                updated_at: record.submitted_at,
            })
        })
        .transpose()
    }

    pub async fn persist_strategy_recovery_snapshot(
        &self,
        snapshot: &PersistedStrategyRecoverySnapshot,
    ) -> Result<(), StateError> {
        let path = self.path.clone();
        let snapshot = snapshot.clone();
        let metrics_json =
            serde_json::to_string(&snapshot.metrics).map_err(|source| StateError::Serialize {
                path: path.clone(),
                source,
            })?;

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO strategy_runtime_states (strategy_id, runtime_state, last_transition_at, updated_at)
                     VALUES (?1, ?2, ?3, ?3)
                     ON CONFLICT(strategy_id) DO UPDATE SET
                        runtime_state = excluded.runtime_state,
                        last_transition_at = excluded.last_transition_at,
                        updated_at = excluded.updated_at",
                    params![snapshot.strategy.strategy_id, snapshot.runtime_state, snapshot.updated_at],
                )?;
                tx.execute(
                    "INSERT INTO strategy_runtime_recovery (strategy_id, runtime_state, next_tick_at, last_event_id, metrics_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(strategy_id) DO UPDATE SET
                        runtime_state = excluded.runtime_state,
                        next_tick_at = excluded.next_tick_at,
                        last_event_id = excluded.last_event_id,
                        metrics_json = excluded.metrics_json,
                        updated_at = excluded.updated_at",
                    params![
                        snapshot.strategy.strategy_id,
                        snapshot.runtime_state,
                        snapshot.next_tick_at,
                        snapshot.last_event_id,
                        metrics_json,
                        snapshot.updated_at,
                    ],
                )?;
                tx.execute(
                    "DELETE FROM strategy_watcher_states WHERE strategy_id = ?1",
                    [snapshot.strategy.strategy_id.as_str()],
                )?;
                for watcher in &snapshot.watcher_states {
                    tx.execute(
                        "INSERT INTO strategy_watcher_states (strategy_id, watcher_key, metric, value, cursor, sampled_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            snapshot.strategy.strategy_id,
                            watcher.watcher_key,
                            watcher.metric,
                            watcher.value,
                            watcher.cursor,
                            watcher.sampled_at,
                        ],
                    )?;
                }
                tx.execute(
                    "DELETE FROM strategy_trigger_memory WHERE strategy_id = ?1",
                    [snapshot.strategy.strategy_id.as_str()],
                )?;
                for trigger in &snapshot.trigger_memory {
                    tx.execute(
                        "INSERT INTO strategy_trigger_memory (strategy_id, trigger_key, cooldown_until, last_fired_at, hysteresis_armed)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            snapshot.strategy.strategy_id,
                            trigger.trigger_key,
                            trigger.cooldown_until,
                            trigger.last_fired_at,
                            if trigger.hysteresis_armed { 1 } else { 0 },
                        ],
                    )?;
                }
                tx.execute(
                    "DELETE FROM strategy_pending_hedges WHERE strategy_id = ?1",
                    [snapshot.strategy.strategy_id.as_str()],
                )?;
                if let Some(hedge) = &snapshot.pending_hedge {
                    tx.execute(
                        "INSERT INTO strategy_pending_hedges (strategy_id, venue, instrument, client_order_id, signer_address, account_address, order_id, nonce, status, last_synced_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            snapshot.strategy.strategy_id,
                            hedge.venue,
                            hedge.instrument,
                            hedge.client_order_id,
                            hedge.signer_address,
                            hedge.account_address,
                            hedge.order_id,
                            hedge.nonce,
                            hedge.status,
                            hedge.last_synced_at,
                        ],
                    )?;
                }
                append_journal_entry(
                    &tx,
                    "strategy",
                    &snapshot.strategy.strategy_id,
                    STRATEGY_EVENT_TYPE,
                    format!(
                        "{{\"strategy_id\":\"{}\",\"runtime_state\":\"{}\",\"updated_at\":\"{}\"}}",
                        snapshot.strategy.strategy_id, snapshot.runtime_state, snapshot.updated_at,
                    ),
                    &snapshot.updated_at,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_strategy_recovery_snapshot(
        &self,
        strategy_id: &str,
    ) -> Result<Option<PersistedStrategyRecoverySnapshot>, StateError> {
        let strategy = match self.load_strategy_registration(strategy_id).await? {
            Some(strategy) => strategy,
            None => return Ok(None),
        };
        self.load_strategy_recovery_snapshot_from_registration(strategy)
            .await
    }

    pub async fn load_runtime_strategy_ids(&self) -> Result<Vec<String>, StateError> {
        let path = self.path.clone();
        self.connection
            .call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT strategy_id FROM strategy_runtime_states ORDER BY updated_at DESC, strategy_id",
                )?;
                stmt.query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_recoverable_strategy_snapshots(
        &self,
    ) -> Result<Vec<PersistedStrategyRecoverySnapshot>, StateError> {
        let path = self.path.clone();
        let strategy_ids = self
            .connection
            .call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT strategy_id FROM strategy_runtime_recovery WHERE runtime_state IN ('active', 'rebalancing', 'syncing_hedge', 'recovering', 'unwinding') ORDER BY strategy_id",
                )?;
                stmt.query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path: path.clone(), source })?;

        let mut snapshots = Vec::new();
        for strategy_id in strategy_ids {
            if let Some(snapshot) = self.load_strategy_recovery_snapshot(&strategy_id).await? {
                snapshots.push(snapshot);
            }
        }
        Ok(snapshots)
    }

    async fn load_strategy_recovery_snapshot_from_registration(
        &self,
        strategy: PersistedStrategyRegistration,
    ) -> Result<Option<PersistedStrategyRecoverySnapshot>, StateError> {
        let path = self.path.clone();
        let strategy_id = strategy.strategy_id.clone();
        let recovery_row = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT runtime_state, next_tick_at, last_event_id, metrics_json, updated_at
                     FROM strategy_runtime_recovery WHERE strategy_id = ?1",
                    [strategy_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        let Some((runtime_state, next_tick_at, last_event_id, metrics_json, updated_at)) =
            recovery_row
        else {
            return Ok(None);
        };

        let watcher_states = self
            .connection
            .call({
                let strategy_id = strategy.strategy_id.clone();
                move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT watcher_key, metric, value, cursor, sampled_at
                         FROM strategy_watcher_states WHERE strategy_id = ?1 ORDER BY watcher_key",
                    )?;
                    stmt.query_map([strategy_id], |row| {
                        Ok(PersistedWatcherState {
                            watcher_key: row.get(0)?,
                            metric: row.get(1)?,
                            value: row.get(2)?,
                            cursor: row.get(3)?,
                            sampled_at: row.get(4)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
                }
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        let trigger_memory = self
            .connection
            .call({
                let strategy_id = strategy.strategy_id.clone();
                move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT trigger_key, cooldown_until, last_fired_at, hysteresis_armed
                         FROM strategy_trigger_memory WHERE strategy_id = ?1 ORDER BY trigger_key",
                    )?;
                    stmt.query_map([strategy_id], |row| {
                        Ok(PersistedTriggerMemory {
                            trigger_key: row.get(0)?,
                            cooldown_until: row.get(1)?,
                            last_fired_at: row.get(2)?,
                            hysteresis_armed: row.get::<_, i64>(3)? != 0,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
                }
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        let pending_hedge = self
            .connection
            .call({
                let strategy_id = strategy.strategy_id.clone();
                move |conn| {
                    conn.query_row(
                        "SELECT venue, instrument, client_order_id, signer_address, account_address, order_id, nonce, status, last_synced_at
                         FROM strategy_pending_hedges WHERE strategy_id = ?1",
                        [strategy_id],
                        |row| {
                            Ok(PersistedPendingHedge {
                                venue: row.get(0)?,
                                instrument: row.get(1)?,
                                client_order_id: row.get(2)?,
                                signer_address: row.get(3)?,
                                account_address: row.get(4)?,
                                order_id: row.get(5)?,
                                nonce: row.get(6)?,
                                status: row.get(7)?,
                                last_synced_at: row.get(8)?,
                            })
                        },
                    )
                    .optional()
                }
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        Ok(Some(PersistedStrategyRecoverySnapshot {
            strategy,
            runtime_state,
            next_tick_at,
            last_event_id,
            metrics: serde_json::from_str(&metrics_json)
                .map_err(|source| StateError::Deserialize { path, source })?,
            watcher_states,
            trigger_memory,
            pending_hedge,
            updated_at,
        }))
    }

    pub async fn persist_route_decision(
        &self,
        request_id: &str,
        source_kind: &str,
        source_id: &str,
        route: &RouteDecision,
        recorded_at: &str,
    ) -> Result<PersistedRouteDecision, StateError> {
        let path = self.path.clone();
        let request_id = request_id.to_owned();
        let source_kind = source_kind.to_owned();
        let source_id = source_id.to_owned();
        let route = route.clone();
        let recorded_at = recorded_at.to_owned();
        let request_id_for_write = request_id.clone();
        let source_kind_for_write = source_kind.clone();
        let source_id_for_write = source_id.clone();
        let recorded_at_for_write = recorded_at.clone();
        let payload_json = serde_json::to_string(&RouteDecisionPayload {
            request_id: request_id.clone(),
            source_kind: source_kind.clone(),
            source_id: source_id.clone(),
            route: route.clone(),
        })
        .map_err(|source| StateError::Serialize {
            path: path.clone(),
            source,
        })?;

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                append_journal_entry(
                    &tx,
                    "agent_request",
                    &request_id_for_write,
                    ROUTE_DECISION_EVENT_TYPE,
                    payload_json.clone(),
                    &recorded_at_for_write,
                )?;
                append_journal_entry(
                    &tx,
                    &source_kind_for_write,
                    &source_id_for_write,
                    ROUTE_DECISION_EVENT_TYPE,
                    payload_json,
                    &recorded_at_for_write,
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist {
                path: path.clone(),
                source,
            })?;

        Ok(PersistedRouteDecision {
            request_id,
            source_kind,
            source_id,
            route,
            recorded_at,
        })
    }

    pub async fn persist_execution_plan(
        &self,
        plan: &ExecutionPlan,
        status: &str,
        recorded_at: &str,
    ) -> Result<PersistedExecutionPlan, StateError> {
        let path = self.path.clone();
        let plan = plan.clone();
        let status = status.to_owned();
        let recorded_at = recorded_at.to_owned();
        let response_plan = plan.clone();
        let response_status = status.clone();
        let response_recorded_at = recorded_at.clone();
        let plan_json = serde_json::to_string(&plan).map_err(|source| StateError::Serialize {
            path: path.clone(),
            source,
        })?;

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO execution_plans (plan_id, source_kind, source_id, request_id, status, summary, plan_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                     ON CONFLICT(plan_id) DO UPDATE SET
                        status = excluded.status,
                        summary = excluded.summary,
                        plan_json = excluded.plan_json,
                        updated_at = excluded.updated_at",
                    params![
                        plan.plan_id,
                        plan.source_kind,
                        plan.source_id,
                        plan.request_id,
                        status,
                        plan.summary,
                        plan_json,
                        recorded_at,
                    ],
                )?;

                append_journal_entry(
                    &tx,
                    "execution_plan",
                    &plan.plan_id,
                    PLAN_CREATED_EVENT_TYPE,
                    serde_json::to_string(&plan)
                        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
                    &recorded_at,
                )?;

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|source| StateError::Persist {
                path: path.clone(),
                source,
            })?;

        Ok(PersistedExecutionPlan {
            plan_id: response_plan.plan_id.clone(),
            source_kind: response_plan.source_kind.clone(),
            source_id: response_plan.source_id.clone(),
            request_id: response_plan.request_id.clone(),
            status: response_status,
            summary: response_plan.summary.clone(),
            plan: response_plan,
            created_at: response_recorded_at.clone(),
            updated_at: response_recorded_at,
        })
    }

    pub async fn persist_execution_plan_step(
        &self,
        step: &PersistedExecutionPlanStep,
    ) -> Result<PersistedExecutionPlanStep, StateError> {
        let path = self.path.clone();
        let step = step.clone();

        self.connection
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO execution_plan_steps (plan_id, step_id, sequence_no, step_type, adapter, idempotency_key, status, attempts, last_error, metadata_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(plan_id, step_id) DO UPDATE SET
                        status = excluded.status,
                        attempts = excluded.attempts,
                        last_error = excluded.last_error,
                        metadata_json = excluded.metadata_json,
                        updated_at = excluded.updated_at",
                    params![
                        step.plan_id,
                        step.step_id,
                        step.sequence_no,
                        step.step_type,
                        step.adapter,
                        step.idempotency_key,
                        step.status,
                        step.attempts,
                        step.last_error,
                        step.metadata_json,
                        step.updated_at,
                    ],
                )?;
                append_journal_entry(
                    &tx,
                    "execution_plan_step",
                    &step.step_id,
                    PLAN_STEP_EVENT_TYPE,
                    serde_json::to_string(&step)
                        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
                    &step.updated_at,
                )?;
                tx.commit()?;
                Ok(step)
            })
            .await
            .map_err(|source| StateError::Persist { path, source })
    }

    pub async fn load_execution_plan(
        &self,
        plan_id: &str,
    ) -> Result<Option<PersistedExecutionPlan>, StateError> {
        let path = self.path.clone();
        let plan_id = plan_id.to_owned();
        let raw = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT plan_id, source_kind, source_id, request_id, status, summary, plan_json, created_at, updated_at
                     FROM execution_plans WHERE plan_id = ?1",
                    [plan_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                        ))
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path: path.clone(), source })?;

        raw.map(
            |(
                plan_id,
                source_kind,
                source_id,
                request_id,
                status,
                summary,
                plan_json,
                created_at,
                updated_at,
            )| {
                Ok(PersistedExecutionPlan {
                    plan: serde_json::from_str(&plan_json).map_err(|source| {
                        StateError::Deserialize {
                            path: path.clone(),
                            source,
                        }
                    })?,
                    plan_id,
                    source_kind,
                    source_id,
                    request_id,
                    status,
                    summary,
                    created_at,
                    updated_at,
                })
            },
        )
        .transpose()
    }

    pub async fn load_execution_plan_steps(
        &self,
        plan_id: &str,
    ) -> Result<Vec<PersistedExecutionPlanStep>, StateError> {
        let path = self.path.clone();
        let plan_id = plan_id.to_owned();
        self.connection
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT plan_id, step_id, sequence_no, step_type, adapter, idempotency_key, status, attempts, last_error, metadata_json, updated_at
                     FROM execution_plan_steps WHERE plan_id = ?1 ORDER BY sequence_no, step_id",
                )?;
                stmt.query_map([plan_id], |row| {
                    Ok(PersistedExecutionPlanStep {
                        plan_id: row.get(0)?,
                        step_id: row.get(1)?,
                        sequence_no: row.get(2)?,
                        step_type: row.get(3)?,
                        adapter: row.get(4)?,
                        idempotency_key: row.get(5)?,
                        status: row.get(6)?,
                        attempts: row.get(7)?,
                        last_error: row.get(8)?,
                        metadata_json: row.get(9)?,
                        updated_at: row.get(10)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_reconciliation_state(
        &self,
        execution_id: &str,
    ) -> Result<Option<ReconciliationStateRecord>, StateError> {
        let path = self.path.clone();
        let execution_id = execution_id.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT execution_id, residual_exposure_usd, rebalance_required, updated_at
                     FROM reconciliation_states
                     WHERE execution_id = ?1",
                    [execution_id],
                    |row| {
                        Ok(ReconciliationStateRecord {
                            execution_id: row.get(0)?,
                            residual_exposure_usd: row.get(1)?,
                            rebalance_required: row.get::<_, i64>(2)? != 0,
                            updated_at: row.get(3)?,
                        })
                    },
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_route_decision(
        &self,
        request_id: &str,
    ) -> Result<Option<PersistedRouteDecision>, StateError> {
        let path = self.path.clone();
        let request_id = request_id.to_owned();

        let raw = self
            .connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT payload_json, created_at
                     FROM event_journal
                     WHERE stream_type = 'agent_request'
                       AND stream_id = ?1
                       AND event_type = ?2
                     ORDER BY rowid DESC
                     LIMIT 1",
                    params![request_id, ROUTE_DECISION_EVENT_TYPE],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load {
                path: path.clone(),
                source,
            })?;

        raw.map(|(payload_json, created_at)| {
            let payload: RouteDecisionPayload =
                serde_json::from_str(&payload_json).map_err(|source| StateError::Deserialize {
                    path: path.clone(),
                    source,
                })?;

            Ok(PersistedRouteDecision {
                request_id: payload.request_id,
                source_kind: payload.source_kind,
                source_id: payload.source_id,
                route: payload.route,
                recorded_at: created_at,
            })
        })
        .transpose()
    }

    pub async fn load_snapshot(&self) -> Result<CanonicalStateSnapshot, StateError> {
        let path = self.path.clone();

        self.connection
            .call(|conn| {
                let mut strategies_stmt = conn.prepare(
                    "SELECT strategy_id, runtime_state, last_transition_at, updated_at
                     FROM strategy_runtime_states
                     ORDER BY strategy_id",
                )?;
                let strategies = strategies_stmt
                    .query_map([], |row| {
                        Ok(StrategyRuntimeStateRecord {
                            strategy_id: row.get(0)?,
                            runtime_state: row.get(1)?,
                            last_transition_at: row.get(2)?,
                            updated_at: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut executions_stmt = conn.prepare(
                    "SELECT execution_id, plan_id, status, updated_at
                     FROM execution_states
                     ORDER BY execution_id",
                )?;
                let executions = executions_stmt
                    .query_map([], |row| {
                        Ok(ExecutionStateRecord {
                            execution_id: row.get(0)?,
                            plan_id: row.get(1)?,
                            status: row.get(2)?,
                            updated_at: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut reconciliations_stmt = conn.prepare(
                    "SELECT execution_id, residual_exposure_usd, rebalance_required, updated_at
                     FROM reconciliation_states
                     ORDER BY execution_id",
                )?;
                let reconciliations = reconciliations_stmt
                    .query_map([], |row| {
                        Ok(ReconciliationStateRecord {
                            execution_id: row.get(0)?,
                            residual_exposure_usd: row.get(1)?,
                            rebalance_required: row.get::<_, i64>(2)? != 0,
                            updated_at: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(CanonicalStateSnapshot {
                    strategies,
                    executions,
                    reconciliations,
                })
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }

    pub async fn load_journal(&self) -> Result<Vec<JournalEntry>, StateError> {
        let path = self.path.clone();

        self.connection
            .call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT event_id, stream_type, stream_id, event_type, payload_json, created_at
                     FROM event_journal
                     ORDER BY rowid",
                )?;
                stmt.query_map([], |row| {
                    Ok(JournalEntry {
                        event_id: row.get(0)?,
                        stream_type: row.get(1)?,
                        stream_id: row.get(2)?,
                        event_type: row.get(3)?,
                        payload_json: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|source| StateError::LoadJournal { path, source })
    }

    pub async fn load_journal_window_for_streams(
        &self,
        stream_filters: &[(String, String)],
        limit: usize,
    ) -> Result<Vec<JournalEntry>, StateError> {
        if stream_filters.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let path = self.path.clone();
        let stream_filters = stream_filters.to_vec();
        let limit = limit.min(i64::MAX as usize) as i64;

        self.connection
            .call(move |conn| {
                let mut query = String::from(
                    "SELECT event_id, stream_type, stream_id, event_type, payload_json, created_at \
                     FROM event_journal WHERE ",
                );
                for (index, _) in stream_filters.iter().enumerate() {
                    if index > 0 {
                        query.push_str(" OR ");
                    }
                    query.push_str("(stream_type = ? AND stream_id = ?)");
                }
                query.push_str(" ORDER BY rowid DESC LIMIT ?");

                let mut params = Vec::with_capacity(stream_filters.len() * 2 + 1);
                for (stream_type, stream_id) in &stream_filters {
                    params.push(stream_type.clone());
                    params.push(stream_id.clone());
                }
                params.push(limit.to_string());

                let mut stmt = conn.prepare(&query)?;
                let mut rows = stmt
                    .query_map(params_from_iter(params.iter()), |row| {
                        Ok(JournalEntry {
                            event_id: row.get(0)?,
                            stream_type: row.get(1)?,
                            stream_id: row.get(2)?,
                            event_type: row.get(3)?,
                            payload_json: row.get(4)?,
                            created_at: row.get(5)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                rows.reverse();
                Ok(rows)
            })
            .await
            .map_err(|source| StateError::LoadJournal { path, source })
    }

    pub async fn last_strategy_transition_at(
        &self,
        strategy_id: &str,
    ) -> Result<Option<String>, StateError> {
        let path = self.path.clone();
        let strategy_id = strategy_id.to_owned();

        self.connection
            .call(move |conn| {
                conn.query_row(
                    "SELECT last_transition_at FROM strategy_runtime_states WHERE strategy_id = ?1",
                    [strategy_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
            })
            .await
            .map_err(|source| StateError::Load { path, source })
    }
}

fn replace_onboarding_checklist_items_tx(
    tx: &rusqlite::Transaction<'_>,
    install_id: &str,
    items: &[PersistedOnboardingChecklistItem],
) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM onboarding_checklist_items WHERE install_id = ?1 AND checklist_key NOT IN (
            SELECT value FROM json_each(?2)
        )",
        params![
            install_id,
            serde_json::to_string(
                &items
                    .iter()
                    .map(|item| item.checklist_key.clone())
                    .collect::<Vec<_>>()
            )
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
        ],
    )?;
    if items.is_empty() {
        tx.execute(
            "DELETE FROM onboarding_checklist_items WHERE install_id = ?1",
            [install_id],
        )?;
    } else {
        for item in items {
            tx.execute(
                "INSERT INTO onboarding_checklist_items (
                    install_id, checklist_key, source_kind, status, blocker_reason, next_action,
                    evidence_json, lifecycle_json, completed_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(install_id, checklist_key) DO UPDATE SET
                    source_kind = excluded.source_kind,
                    status = excluded.status,
                    blocker_reason = excluded.blocker_reason,
                    next_action = excluded.next_action,
                    evidence_json = excluded.evidence_json,
                    lifecycle_json = excluded.lifecycle_json,
                    completed_at = excluded.completed_at,
                    updated_at = excluded.updated_at",
                params![
                    item.install_id,
                    item.checklist_key,
                    item.source_kind,
                    item.status,
                    item.blocker_reason,
                    item.next_action,
                    serde_json::to_string(&item.evidence).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?,
                    item.lifecycle
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })?,
                    item.completed_at,
                    item.created_at,
                    item.updated_at,
                ],
            )?;
        }
    }

    Ok(())
}

fn replace_onboarding_route_readiness_steps_tx(
    tx: &rusqlite::Transaction<'_>,
    install_id: &str,
    proposal_id: &str,
    route_id: &str,
    steps: &[PersistedRouteReadinessStep],
) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM onboarding_route_readiness_steps
         WHERE install_id = ?1 AND proposal_id = ?2 AND route_id = ?3 AND step_key NOT IN (
            SELECT value FROM json_each(?4)
         )",
        params![
            install_id,
            proposal_id,
            route_id,
            serde_json::to_string(
                &steps
                    .iter()
                    .map(|item| item.step_key.clone())
                    .collect::<Vec<_>>()
            )
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
        ],
    )?;
    if steps.is_empty() {
        tx.execute(
            "DELETE FROM onboarding_route_readiness_steps
             WHERE install_id = ?1 AND proposal_id = ?2 AND route_id = ?3",
            params![install_id, proposal_id, route_id],
        )?;
    } else {
        for step in steps {
            tx.execute(
                "INSERT INTO onboarding_route_readiness_steps (
                    install_id, proposal_id, route_id, step_key, status, blocker_reason,
                    recommended_action_json, completed_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(install_id, proposal_id, route_id, step_key) DO UPDATE SET
                    status = excluded.status,
                    blocker_reason = excluded.blocker_reason,
                    recommended_action_json = excluded.recommended_action_json,
                    completed_at = excluded.completed_at,
                    updated_at = excluded.updated_at",
                params![
                    step.install_id,
                    step.proposal_id,
                    step.route_id,
                    step.step_key,
                    step.status,
                    step.blocker_reason,
                    step.recommended_action
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })?,
                    step.completed_at,
                    step.created_at,
                    step.updated_at,
                ],
            )?;
        }
    }

    Ok(())
}

fn ensure_strategy_pending_hedge_columns(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    for statement in [
        "ALTER TABLE strategy_pending_hedges ADD COLUMN signer_address TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE strategy_pending_hedges ADD COLUMN account_address TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE strategy_pending_hedges ADD COLUMN order_id INTEGER",
    ] {
        match conn.execute(statement, []) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(error, _)) if error.extended_code == 1 => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn ensure_onboarding_install_columns(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    for statement in [
        "ALTER TABLE onboarding_installs ADD COLUMN onboarding_status TEXT NOT NULL DEFAULT 'blocked'",
        "ALTER TABLE onboarding_installs ADD COLUMN bundle_drift_json TEXT",
        "ALTER TABLE onboarding_installs ADD COLUMN last_onboarding_rejection_code TEXT",
        "ALTER TABLE onboarding_installs ADD COLUMN last_onboarding_rejection_message TEXT",
        "ALTER TABLE onboarding_installs ADD COLUMN last_onboarding_rejection_at TEXT",
    ] {
        match conn.execute(statement, []) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(error, _)) if error.extended_code == 1 => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn ensure_onboarding_route_readiness_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    for statement in [
        "ALTER TABLE onboarding_route_readiness ADD COLUMN ordered_steps_json TEXT NOT NULL DEFAULT '[]'",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN current_step_key TEXT",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN last_route_rejection_code TEXT",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN last_route_rejection_message TEXT",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN last_route_rejection_at TEXT",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN evaluation_fingerprint TEXT",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN stale_status TEXT NOT NULL DEFAULT 'fresh'",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN stale_reason TEXT",
        "ALTER TABLE onboarding_route_readiness ADD COLUMN stale_detected_at TEXT",
    ] {
        match conn.execute(statement, []) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(error, _)) if error.extended_code == 1 => {}
            Err(error) => return Err(error),
        }
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS onboarding_route_readiness_steps (
            install_id TEXT NOT NULL,
            proposal_id TEXT NOT NULL,
            route_id TEXT NOT NULL,
            step_key TEXT NOT NULL,
            status TEXT NOT NULL,
            blocker_reason TEXT,
            recommended_action_json TEXT,
            completed_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (install_id, proposal_id, route_id, step_key),
            FOREIGN KEY(install_id, proposal_id, route_id)
                REFERENCES onboarding_route_readiness(install_id, proposal_id, route_id)
                ON DELETE CASCADE
        );",
    )?;
    Ok(())
}

fn ensure_strategy_selection_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    for statement in [
        "ALTER TABLE onboarding_strategy_selections ADD COLUMN reopened_from_revision INTEGER",
        "ALTER TABLE onboarding_strategy_selections ADD COLUMN approval_stale INTEGER NOT NULL DEFAULT 0 CHECK (approval_stale IN (0, 1))",
        "ALTER TABLE onboarding_strategy_selections ADD COLUMN approval_stale_reason TEXT",
    ] {
        match conn.execute(statement, []) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("duplicate column name") => {}
            Err(error) => return Err(error),
        }
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS onboarding_strategy_selection_approval_history (
            history_id INTEGER PRIMARY KEY AUTOINCREMENT,
            install_id TEXT NOT NULL,
            proposal_id TEXT NOT NULL,
            selection_id TEXT NOT NULL,
            event_kind TEXT NOT NULL,
            selection_revision INTEGER NOT NULL,
            approved_revision INTEGER,
            reopened_from_revision INTEGER,
            approved_by TEXT,
            note TEXT,
            reason TEXT,
            provenance_json TEXT,
            created_at TEXT NOT NULL,
            FOREIGN KEY(selection_id) REFERENCES onboarding_strategy_selections(selection_id) ON DELETE CASCADE
        );",
    )?;
    Ok(())
}

fn ensure_strategy_runtime_handoff_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS onboarding_strategy_runtime_handoffs (
            install_id TEXT NOT NULL,
            proposal_id TEXT NOT NULL,
            selection_id TEXT NOT NULL,
            approved_selection_revision INTEGER NOT NULL,
            route_id TEXT NOT NULL,
            request_id TEXT NOT NULL,
            route_readiness_fingerprint TEXT NOT NULL,
            route_readiness_status TEXT NOT NULL,
            route_readiness_evaluated_at TEXT NOT NULL,
            eligibility_status TEXT NOT NULL,
            hold_reason TEXT,
            runtime_control_mode TEXT NOT NULL,
            strategy_id TEXT,
            runtime_identity_refreshed_at TEXT,
            runtime_identity_source TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (install_id, proposal_id, selection_id),
            FOREIGN KEY(selection_id) REFERENCES onboarding_strategy_selections(selection_id) ON DELETE CASCADE
        );",
    )?;
    for statement in [
        "ALTER TABLE onboarding_strategy_runtime_handoffs ADD COLUMN runtime_identity_refreshed_at TEXT",
        "ALTER TABLE onboarding_strategy_runtime_handoffs ADD COLUMN runtime_identity_source TEXT",
    ] {
        match conn.execute(statement, []) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("duplicate column name") => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn ensure_runtime_control_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS runtime_control (
            scope_key TEXT PRIMARY KEY,
            control_mode TEXT NOT NULL,
            transition_reason TEXT NOT NULL,
            transition_source TEXT NOT NULL,
            transitioned_at TEXT NOT NULL,
            last_cleared_at TEXT,
            last_cleared_reason TEXT,
            last_cleared_source TEXT,
            last_rejection_code TEXT,
            last_rejection_message TEXT,
            last_rejection_operation TEXT,
            last_rejection_at TEXT,
            updated_at TEXT NOT NULL
        );",
    )?;
    Ok(())
}

fn append_journal_entry(
    tx: &rusqlite::Transaction<'_>,
    stream_type: &str,
    stream_id: &str,
    event_type: &str,
    payload_json: String,
    created_at: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO event_journal (event_id, stream_type, stream_id, event_type, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            Uuid::now_v7().to_string(),
            stream_type,
            stream_id,
            event_type,
            payload_json,
            created_at,
        ],
    )?;

    Ok(())
}
