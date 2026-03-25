use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a2ex_control::{AgentRequestEnvelope, Intent};
use a2ex_daemon::{
    project_route_support_truth, project_strategy_runtime_monitoring,
    project_strategy_runtime_reconciliation,
};
use a2ex_skill_bundle::{
    BundleDocumentLifecycleChangeKind, BundleLifecycleClassification, BundleLoadOutcome,
    InterpretationBlocker, InterpretationEvidence, InterpretationOwnerDecision,
    InterpretationSetupRequirement, InterpretationSetupRequirementKind,
    ProposalQuantitativeCompleteness, SkillBundleInterpretation, SkillProposalPacket,
};
use a2ex_state::{
    AUTONOMOUS_RUNTIME_CONTROL_SCOPE, JournalEntry, PersistedOnboardingChecklistItem,
    PersistedOnboardingInstall, PersistedOnboardingWorkspace, PersistedRouteReadiness,
    PersistedRouteReadinessStep, PersistedRuntimeControl, PersistedStrategyRuntimeHandoff,
    PersistedStrategySelection, PersistedStrategySelectionApprovalHistoryEvent,
    PersistedStrategySelectionOverride, StateRepository,
};
use reqwest::Url;
use sha2::{Digest, Sha256};

use crate::model::{
    ApplyStrategySelectionOverride, ApproveStrategySelectionRequest, BootstrapAttemptMetadata,
    BootstrapFailureSummary, BootstrapReport, BootstrapSource, ClaimDisposition,
    GuidedOnboardingAction, GuidedOnboardingActionKind, GuidedOnboardingActionRef,
    GuidedOnboardingActionRejection, GuidedOnboardingActionResult, GuidedOnboardingError,
    GuidedOnboardingInspection, GuidedOnboardingState, GuidedOnboardingStep, InstallBootstrapError,
    InstallReadiness, MaterializeStrategySelectionRequest, OnboardingAggregateStatus,
    OnboardingBundleDrift, OnboardingChecklistItem, OnboardingChecklistItemStatus,
    OnboardingChecklistSourceKind, OnboardingEvidenceKind, OnboardingEvidenceRef, OnboardingView,
    ProposalHandoff, ReopenStrategySelectionRequest, RouteApprovalTuple, RouteCapitalReadiness,
    RouteOwnerAction, RouteReadinessAction, RouteReadinessActionKind, RouteReadinessActionRef,
    RouteReadinessActionRejection, RouteReadinessActionResult, RouteReadinessBlocker,
    RouteReadinessError, RouteReadinessEvaluationMetadata, RouteReadinessIdentity,
    RouteReadinessRecord, RouteReadinessStaleState, RouteReadinessStaleStatus,
    RouteReadinessStatus, RouteReadinessStep, RouteReadinessStepStatus, RuntimeControlSnapshot,
    STRATEGY_EXCEPTION_ROLLUP_KIND, STRATEGY_OPERATOR_REPORT_KIND, STRATEGY_REPORT_WINDOW_KIND,
    StrategyExceptionRollup, StrategyExceptionUrgency, StrategyOperatorReconciliationEvidence,
    StrategyOperatorReconciliationStatus, StrategyOperatorReport, StrategyOperatorReportFreshness,
    StrategyReportWindow, StrategyReportWindowChange, StrategyReportWindowChangeKind,
    StrategyReportWindowIdentity, StrategyRuntimeActionView, StrategyRuntimeControlGateStatus,
    StrategyRuntimeEligibilityStatus, StrategyRuntimeHandoffError, StrategyRuntimeHandoffRecord,
    StrategyRuntimeHoldException, StrategyRuntimeHoldReason, StrategyRuntimeLastOutcome,
    StrategyRuntimeMonitoringSummary, StrategyRuntimeOperatorGuidance, StrategyRuntimePhase,
    StrategySelectionApprovalHistoryEvent, StrategySelectionApprovalState,
    StrategySelectionDiscussion, StrategySelectionEffectiveDiff, StrategySelectionError,
    StrategySelectionInspection, StrategySelectionOverrideRecord,
    StrategySelectionReadinessSensitivitySummary, StrategySelectionRecommendationBasis,
    StrategySelectionRecord, StrategySelectionSensitivityClass, StrategySelectionStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceClaimRecord {
    pub workspace_id: String,
    pub install_id: String,
    pub claim_disposition: ClaimDisposition,
    pub canonical_workspace_root: PathBuf,
    pub attached_bundle_url: Url,
    pub attempt: BootstrapAttemptMetadata,
    pub readiness: InstallReadiness,
}

const BUNDLE_STATE_CHECKLIST_KEY: &str = "__bundle_state__";

#[derive(Debug)]
pub struct OnboardingStore {
    repository: StateRepository,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedChecklistLifecycle {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    source_document_ids: Vec<String>,
    load_outcome: BundleLoadOutcome,
}

impl OnboardingStore {
    pub fn new(repository: StateRepository) -> Self {
        Self { repository }
    }

    pub async fn open(state_db_path: impl AsRef<Path>) -> Result<Self, InstallBootstrapError> {
        let state_db_path = state_db_path.as_ref();
        if let Some(parent) = state_db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                InstallBootstrapError::Persistence {
                    message: error.to_string(),
                }
            })?;
        }
        let repository = StateRepository::open(state_db_path)
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?;
        Ok(Self::new(repository))
    }

    pub async fn claim_workspace_install(
        &self,
        workspace_root: &Path,
        install_url: &Url,
        expected_workspace_id: Option<&str>,
        expected_install_id: Option<&str>,
    ) -> Result<WorkspaceClaimRecord, InstallBootstrapError> {
        let canonical_workspace_root = canonical_workspace_root(workspace_root)?;
        let canonical_workspace_root_string =
            canonical_workspace_root.to_string_lossy().into_owned();
        let now = current_timestamp();

        let workspace = match self
            .repository
            .load_onboarding_workspace_by_root(&canonical_workspace_root_string)
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })? {
            Some(existing) => existing,
            None => {
                let workspace_id = deterministic_id("ws", &[&canonical_workspace_root_string]);
                if expected_workspace_id.is_some_and(|expected| expected != workspace_id) {
                    return Err(InstallBootstrapError::WorkspaceIdentityMismatch {
                        existing_workspace_id: workspace_id.clone(),
                        existing_install_id: deterministic_id(
                            "install",
                            &[&workspace_id, install_url.as_str()],
                        ),
                    });
                }
                let workspace = PersistedOnboardingWorkspace {
                    workspace_id,
                    canonical_workspace_root: canonical_workspace_root_string.clone(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                };
                self.repository
                    .persist_onboarding_workspace(&workspace)
                    .await
                    .map_err(|error| InstallBootstrapError::Persistence {
                        message: error.to_string(),
                    })?;
                workspace
            }
        };

        let expected_install_id = expected_install_id.map(ToOwned::to_owned);
        if let Some(expected_workspace_id) = expected_workspace_id
            && expected_workspace_id != workspace.workspace_id
        {
            let existing_install_id = self
                .repository
                .load_onboarding_install(&workspace.workspace_id, install_url.as_str())
                .await
                .map_err(|error| InstallBootstrapError::Persistence {
                    message: error.to_string(),
                })?
                .map(|record| record.install_id)
                .unwrap_or_else(|| {
                    deterministic_id("install", &[&workspace.workspace_id, install_url.as_str()])
                });
            return Err(InstallBootstrapError::WorkspaceIdentityMismatch {
                existing_workspace_id: workspace.workspace_id,
                existing_install_id,
            });
        }

        let existing_install = self
            .repository
            .load_onboarding_install(&workspace.workspace_id, install_url.as_str())
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?;

        let (install, claim_disposition) = match existing_install {
            Some(existing) => {
                if expected_install_id
                    .as_deref()
                    .is_some_and(|expected| expected != existing.install_id)
                {
                    return Err(InstallBootstrapError::InstallIdentityMismatch {
                        existing_workspace_id: workspace.workspace_id,
                        existing_install_id: existing.install_id,
                    });
                }
                (existing, ClaimDisposition::Reopened)
            }
            None => {
                let install_id =
                    deterministic_id("install", &[&workspace.workspace_id, install_url.as_str()]);
                if expected_install_id
                    .as_deref()
                    .is_some_and(|expected| expected != install_id)
                {
                    return Err(InstallBootstrapError::InstallIdentityMismatch {
                        existing_workspace_id: workspace.workspace_id.clone(),
                        existing_install_id: install_id,
                    });
                }
                let readiness = InstallReadiness::blocked_placeholder();
                let install = PersistedOnboardingInstall {
                    install_id,
                    workspace_id: workspace.workspace_id.clone(),
                    install_url: install_url.to_string(),
                    attached_bundle_url: install_url.to_string(),
                    claim_disposition: ClaimDisposition::Claimed.as_str().to_owned(),
                    bootstrap_source: BootstrapSource::LocalRuntime.as_str().to_owned(),
                    bootstrap_path: "pending".to_owned(),
                    state_db_path: canonical_workspace_root
                        .join(".a2ex-daemon/state.db")
                        .display()
                        .to_string(),
                    analytics_db_path: canonical_workspace_root
                        .join(".a2ex-daemon/analytics.db")
                        .display()
                        .to_string(),
                    used_remote_control_plane: false,
                    recovered_existing_state: false,
                    bootstrap_attempt_count: 0,
                    last_bootstrap_attempt_at: None,
                    last_bootstrap_completed_at: None,
                    last_bootstrap_failure_stage: None,
                    last_bootstrap_failure_summary: None,
                    readiness_status: readiness.status_as_str().to_owned(),
                    readiness_blockers: readiness.blockers.clone(),
                    readiness_diagnostics: readiness.diagnostics.clone(),
                    onboarding_status: OnboardingAggregateStatus::Blocked.as_str().to_owned(),
                    bundle_drift: None,
                    last_onboarding_rejection_code: None,
                    last_onboarding_rejection_message: None,
                    last_onboarding_rejection_at: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                };
                self.repository
                    .persist_onboarding_install(&install)
                    .await
                    .map_err(|error| InstallBootstrapError::Persistence {
                        message: error.to_string(),
                    })?;
                (install, ClaimDisposition::Claimed)
            }
        };

        Ok(WorkspaceClaimRecord {
            workspace_id: workspace.workspace_id,
            install_id: install.install_id,
            claim_disposition,
            canonical_workspace_root,
            attached_bundle_url: parse_url(&install.attached_bundle_url)?,
            attempt: BootstrapAttemptMetadata {
                attempt_count: install.bootstrap_attempt_count,
                last_attempt_at: install.last_bootstrap_attempt_at,
                last_completed_at: install.last_bootstrap_completed_at,
                last_failure: install
                    .last_bootstrap_failure_stage
                    .zip(install.last_bootstrap_failure_summary)
                    .map(|(stage, message)| BootstrapFailureSummary { stage, message }),
            },
            readiness: InstallReadiness {
                status: InstallReadiness::status_from_str(&install.readiness_status).ok_or_else(
                    || InstallBootstrapError::Persistence {
                        message: format!(
                            "unknown persisted readiness status {}",
                            install.readiness_status
                        ),
                    },
                )?,
                blockers: install.readiness_blockers,
                diagnostics: install.readiness_diagnostics,
            },
        })
    }

    pub async fn persist_bootstrap_success(
        &self,
        claim: &WorkspaceClaimRecord,
        bootstrap: &BootstrapReport,
        attached_bundle_url: &Url,
        readiness: &InstallReadiness,
    ) -> Result<WorkspaceClaimRecord, InstallBootstrapError> {
        let mut install = self
            .repository
            .load_onboarding_install(&claim.workspace_id, claim.attached_bundle_url.as_str())
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| InstallBootstrapError::Persistence {
                message: format!(
                    "missing persisted install {} for workspace {}",
                    claim.install_id, claim.workspace_id
                ),
            })?;
        let now = current_timestamp();
        install.claim_disposition = claim.claim_disposition.as_str().to_owned();
        install.attached_bundle_url = attached_bundle_url.to_string();
        install.bootstrap_source = bootstrap.source.as_str().to_owned();
        install.bootstrap_path = bootstrap.bootstrap_path.clone();
        install.state_db_path = bootstrap.state_db_path.display().to_string();
        install.analytics_db_path = bootstrap.analytics_db_path.display().to_string();
        install.used_remote_control_plane = bootstrap.used_remote_control_plane;
        install.recovered_existing_state = bootstrap.recovered_existing_state;
        install.bootstrap_attempt_count = install.bootstrap_attempt_count.saturating_add(1);
        install.last_bootstrap_attempt_at = Some(now.clone());
        install.last_bootstrap_completed_at = Some(now.clone());
        install.last_bootstrap_failure_stage = None;
        install.last_bootstrap_failure_summary = None;
        install.readiness_status = readiness.status_as_str().to_owned();
        install.readiness_blockers = readiness.blockers.clone();
        install.readiness_diagnostics = readiness.diagnostics.clone();
        install.updated_at = now;
        self.repository
            .persist_onboarding_install(&install)
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?;

        Ok(WorkspaceClaimRecord {
            workspace_id: install.workspace_id,
            install_id: install.install_id,
            claim_disposition: claim.claim_disposition,
            canonical_workspace_root: claim.canonical_workspace_root.clone(),
            attached_bundle_url: parse_url(&install.attached_bundle_url)?,
            attempt: BootstrapAttemptMetadata {
                attempt_count: install.bootstrap_attempt_count,
                last_attempt_at: install.last_bootstrap_attempt_at,
                last_completed_at: install.last_bootstrap_completed_at,
                last_failure: None,
            },
            readiness: readiness.clone(),
        })
    }

    pub async fn persist_interpreted_onboarding(
        &self,
        install_id: &str,
        interpretation: &SkillBundleInterpretation,
        load_outcome: &BundleLoadOutcome,
    ) -> Result<OnboardingView, InstallBootstrapError> {
        let mut install = self
            .repository
            .load_onboarding_install_by_id(install_id)
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| InstallBootstrapError::Persistence {
                message: format!("missing persisted install {install_id}"),
            })?;
        let existing_items = self
            .repository
            .load_onboarding_checklist_items(install_id)
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?;
        let visible_existing_items = existing_items
            .iter()
            .filter(|item| !is_bundle_state_item(item))
            .cloned()
            .collect::<Vec<_>>();

        let previous_load_outcome = previous_load_outcome(&existing_items)?;
        let drift = bundle_drift(previous_load_outcome.as_ref(), load_outcome);
        let projected_items = project_checklist_items(interpretation, drift.as_ref());
        let merged_items = merge_with_existing_items(projected_items, visible_existing_items)?;
        let aggregate_status = derive_aggregate_status(
            &readiness_from_install(&install)?,
            &merged_items,
            drift.as_ref(),
        );
        let now = current_timestamp();
        let mut persisted_items = merged_items
            .iter()
            .map(|item| persisted_checklist_item(install_id, item, load_outcome, &now))
            .collect::<Result<Vec<_>, _>>()?;
        persisted_items.push(bundle_state_persisted_item(install_id, load_outcome, &now)?);

        install.onboarding_status = aggregate_status.as_str().to_owned();
        install.bundle_drift = drift
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?;
        install.updated_at = now.clone();
        self.repository
            .persist_onboarding_install_state(&install, &persisted_items)
            .await
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?;

        Ok(OnboardingView::from_items(
            aggregate_status,
            merged_items,
            drift,
        ))
    }

    pub async fn read_guided_onboarding(
        &self,
        install_id: &str,
    ) -> Result<GuidedOnboardingState, GuidedOnboardingError> {
        let (install, persisted_items) = self.load_guided_install_state(install_id).await?;
        guided_state_from_parts(&install, &persisted_items)
    }

    pub async fn inspect_guided_onboarding(
        &self,
        install_id: &str,
    ) -> Result<GuidedOnboardingInspection, GuidedOnboardingError> {
        let (install, persisted_items) = self.load_guided_install_state(install_id).await?;
        guided_inspection_from_parts(&install, &persisted_items)
    }

    pub async fn apply_guided_onboarding_action(
        &self,
        install_id: &str,
        action: GuidedOnboardingAction,
    ) -> Result<GuidedOnboardingActionResult, GuidedOnboardingError> {
        let (mut install, mut persisted_items) = self.load_guided_install_state(install_id).await?;
        let guided = guided_state_from_parts(&install, &persisted_items)?;
        let current_step_key = guided.current_step_key.clone();

        if matches!(action, GuidedOnboardingAction::Refresh) {
            return Ok(guided.into_action_result());
        }

        let mutation_result = (|| match action {
            GuidedOnboardingAction::Refresh => Ok(()),
            GuidedOnboardingAction::AcknowledgeBundleDrift => {
                validate_current_step(
                    current_step_key.as_deref(),
                    Some("bundle_drift"),
                    "bundle_drift_review_required",
                    "bundle drift review must be acknowledged before later onboarding steps can proceed",
                )?;
                let drift_item = persisted_items
                    .iter_mut()
                    .find(|item| !is_bundle_state_item(item) && item.checklist_key == "bundle_drift")
                    .ok_or_else(|| GuidedOnboardingError::ActionRejected {
                        code: "bundle_drift_review_required".to_owned(),
                        message:
                            "bundle drift review must be acknowledged before later onboarding steps can proceed"
                                .to_owned(),
                    })?;
                let now = current_timestamp();
                drift_item.status = OnboardingChecklistItemStatus::Completed.as_str().to_owned();
                drift_item.completed_at = Some(now.clone());
                drift_item.updated_at = now.clone();
                install.bundle_drift = None;
                install.onboarding_status =
                    derive_guided_aggregate_status(&install, &persisted_items, None)
                        .as_str()
                        .to_owned();
                install.updated_at = now;
                Ok(())
            }
            GuidedOnboardingAction::CompleteStep { step_key } => {
                validate_current_step(
                    current_step_key.as_deref(),
                    Some(step_key.as_str()),
                    mismatch_action_code(current_step_key.as_deref()),
                    mismatch_action_message(current_step_key.as_deref()),
                )?;
                let item = persisted_items
                    .iter_mut()
                    .find(|item| !is_bundle_state_item(item) && item.checklist_key == step_key)
                    .ok_or_else(|| GuidedOnboardingError::ActionRejected {
                        code: "step_not_found".to_owned(),
                        message: format!("guided onboarding step '{step_key}' was not found"),
                    })?;
                if item.source_kind != OnboardingChecklistSourceKind::SetupRequirement.as_str() {
                    Err(GuidedOnboardingError::ActionRejected {
                        code: "step_not_completable".to_owned(),
                        message: format!(
                            "guided onboarding step '{step_key}' is not a local setup step"
                        ),
                    })
                } else if item.status != OnboardingChecklistItemStatus::Pending.as_str() {
                    Err(GuidedOnboardingError::ActionRejected {
                        code: "step_not_pending".to_owned(),
                        message: format!(
                            "guided onboarding step '{step_key}' is not pending and cannot be completed"
                        ),
                    })
                } else {
                    let now = current_timestamp();
                    item.status = OnboardingChecklistItemStatus::Completed.as_str().to_owned();
                    item.completed_at = Some(now.clone());
                    item.updated_at = now.clone();
                    install.onboarding_status = derive_guided_aggregate_status(
                        &install,
                        &persisted_items,
                        guided.drift.as_ref(),
                    )
                    .as_str()
                    .to_owned();
                    install.updated_at = now;
                    Ok(())
                }
            }
            GuidedOnboardingAction::ResolveOwnerDecision { step_key, .. } => {
                validate_current_step(
                    current_step_key.as_deref(),
                    Some(step_key.as_str()),
                    mismatch_action_code(current_step_key.as_deref()),
                    mismatch_action_message(current_step_key.as_deref()),
                )?;
                let item = persisted_items
                    .iter_mut()
                    .find(|item| !is_bundle_state_item(item) && item.checklist_key == step_key)
                    .ok_or_else(|| GuidedOnboardingError::ActionRejected {
                        code: "step_not_found".to_owned(),
                        message: format!("guided onboarding step '{step_key}' was not found"),
                    })?;
                if item.source_kind != OnboardingChecklistSourceKind::OwnerDecision.as_str() {
                    Err(GuidedOnboardingError::ActionRejected {
                        code: "owner_decision_required".to_owned(),
                        message: format!(
                            "guided onboarding step '{step_key}' is not an owner decision"
                        ),
                    })
                } else if item.status != OnboardingChecklistItemStatus::Pending.as_str() {
                    Err(GuidedOnboardingError::ActionRejected {
                        code: "step_not_pending".to_owned(),
                        message: format!(
                            "guided onboarding step '{step_key}' is not pending and cannot be resolved"
                        ),
                    })
                } else {
                    let now = current_timestamp();
                    item.status = OnboardingChecklistItemStatus::Completed.as_str().to_owned();
                    item.completed_at = Some(now.clone());
                    item.updated_at = now.clone();
                    install.onboarding_status = derive_guided_aggregate_status(
                        &install,
                        &persisted_items,
                        guided.drift.as_ref(),
                    )
                    .as_str()
                    .to_owned();
                    install.updated_at = now;
                    Ok(())
                }
            }
        })();

        if let Err(GuidedOnboardingError::ActionRejected { code, message }) = &mutation_result {
            install.last_onboarding_rejection_code = Some(code.clone());
            install.last_onboarding_rejection_message = Some(message.clone());
            install.last_onboarding_rejection_at = Some(current_timestamp());
            self.repository
                .persist_onboarding_install_state(&install, &persisted_items)
                .await
                .map_err(|error| GuidedOnboardingError::Persistence {
                    message: error.to_string(),
                })?;
            return Err(GuidedOnboardingError::ActionRejected {
                code: code.clone(),
                message: message.clone(),
            });
        }
        mutation_result?;
        install.last_onboarding_rejection_code = None;
        install.last_onboarding_rejection_message = None;
        install.last_onboarding_rejection_at = None;

        self.repository
            .persist_onboarding_install_state(&install, &persisted_items)
            .await
            .map_err(|error| GuidedOnboardingError::Persistence {
                message: error.to_string(),
            })?;

        guided_state_from_parts(&install, &persisted_items)
            .map(GuidedOnboardingState::into_action_result)
    }

    pub async fn evaluate_route_readiness(
        &self,
        install_id: &str,
        proposal_id: &str,
        route_id: &str,
        request_id: &str,
    ) -> Result<RouteReadinessRecord, RouteReadinessError> {
        let install = self
            .repository
            .load_onboarding_install_by_id(install_id)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| RouteReadinessError::Persistence {
                message: format!("missing persisted install {install_id}"),
            })?;
        let envelope = self
            .repository
            .load_intent_envelope(request_id)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| RouteReadinessError::Persistence {
                message: format!("missing persisted intent envelope for request {request_id}"),
            })?;
        let route_decision = self
            .repository
            .load_route_decision(request_id)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| RouteReadinessError::Persistence {
                message: format!("missing persisted route decision for request {request_id}"),
            })?;
        let reserved_capital_usd = self
            .repository
            .load_capital_reservations_for_execution(request_id)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?
            .into_iter()
            .filter(|reservation| reservation.state == "held")
            .map(|reservation| reservation.amount)
            .sum::<u64>();
        let existing_state = self
            .repository
            .load_onboarding_route_readiness_state(install_id, proposal_id, route_id)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: format!("{error:?}"),
            })?;

        let evaluated_at = current_timestamp();
        let base_record = build_route_readiness_record(
            &install,
            &envelope,
            &route_decision.route,
            proposal_id,
            route_id,
            request_id,
            (reserved_capital_usd > 0).then_some(reserved_capital_usd),
            &evaluated_at,
        )?;
        let record =
            route_record_with_guided_state(base_record, existing_state.as_ref(), &evaluated_at)?;
        let (persisted, steps) =
            persisted_route_readiness_state_from_record(&record, &evaluated_at)?;
        self.repository
            .persist_onboarding_route_readiness_state(&persisted, &steps)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?;

        self.inspect_route_readiness(install_id, proposal_id, route_id)
            .await
    }

    pub async fn inspect_route_readiness(
        &self,
        install_id: &str,
        proposal_id: &str,
        route_id: &str,
    ) -> Result<RouteReadinessRecord, RouteReadinessError> {
        let (persisted, persisted_steps) = self
            .repository
            .load_onboarding_route_readiness_state(install_id, proposal_id, route_id)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: format!("{error:?}"),
            })?
            .ok_or_else(|| RouteReadinessError::Persistence {
                message: format!(
                    "missing persisted route readiness for install {install_id} proposal {proposal_id} route {route_id}"
                ),
            })?;
        route_readiness_from_persisted(&persisted, &persisted_steps)
    }

    pub async fn apply_route_readiness_action(
        &self,
        install_id: &str,
        proposal_id: &str,
        route_id: &str,
        action: RouteReadinessAction,
    ) -> Result<RouteReadinessActionResult, RouteReadinessError> {
        let (mut summary, mut steps) = self
            .repository
            .load_onboarding_route_readiness_state(install_id, proposal_id, route_id)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: format!("{error:?}"),
            })?
            .ok_or_else(|| RouteReadinessError::Persistence {
                message: format!(
                    "missing persisted route readiness for install {install_id} proposal {proposal_id} route {route_id}"
                ),
            })?;
        let record = route_readiness_from_persisted(&summary, &steps)?;
        let current_step_key = record.current_step_key.clone();

        if matches!(action, RouteReadinessAction::Refresh) {
            return Ok(record.into_action_result());
        }

        let mutation_result = (|| match action {
            RouteReadinessAction::Refresh => Ok(()),
            RouteReadinessAction::ReviewStaleReadiness => {
                validate_route_current_step(
                    current_step_key.as_deref(),
                    Some("review_stale_readiness"),
                    route_mismatch_action_code(current_step_key.as_deref()),
                    route_mismatch_action_message(current_step_key.as_deref()),
                )?;
                let now = current_timestamp();
                for step in &mut steps {
                    if step.step_key == "review_stale_readiness" {
                        step.status = RouteReadinessStepStatus::Completed.as_str().to_owned();
                        step.completed_at = Some(now.clone());
                        step.updated_at = now.clone();
                    } else if step.status == RouteReadinessStepStatus::Stale.as_str() {
                        step.status = RouteReadinessStepStatus::Pending.as_str().to_owned();
                        step.completed_at = None;
                        step.updated_at = now.clone();
                    }
                }
                summary.stale_status = RouteReadinessStaleStatus::Fresh.as_str().to_owned();
                summary.stale_reason = None;
                summary.stale_detected_at = None;
                summary.updated_at = now;
                Ok(())
            }
            RouteReadinessAction::CompleteStep { step_key } => {
                validate_route_current_step(
                    current_step_key.as_deref(),
                    Some(step_key.as_str()),
                    route_mismatch_action_code(current_step_key.as_deref()),
                    route_mismatch_action_message(current_step_key.as_deref()),
                )?;
                let step = steps
                    .iter_mut()
                    .find(|step| step.step_key == step_key)
                    .ok_or_else(|| RouteReadinessError::ActionRejected {
                        code: "step_not_found".to_owned(),
                        message: format!("route readiness step '{step_key}' was not found"),
                    })?;
                if step.status == RouteReadinessStepStatus::Completed.as_str() {
                    return Err(RouteReadinessError::ActionRejected {
                        code: "step_not_pending".to_owned(),
                        message: format!("route readiness step '{step_key}' is already completed"),
                    });
                }
                if step.status == RouteReadinessStepStatus::Stale.as_str() {
                    return Err(RouteReadinessError::ActionRejected {
                        code: "stale_review_required".to_owned(),
                        message:
                            "stale route readiness must be reviewed before later steps can proceed"
                                .to_owned(),
                    });
                }
                let now = current_timestamp();
                step.status = RouteReadinessStepStatus::Completed.as_str().to_owned();
                step.completed_at = Some(now.clone());
                step.updated_at = now.clone();
                summary.updated_at = now;
                Ok(())
            }
        })();

        if let Err(RouteReadinessError::ActionRejected { code, message }) = &mutation_result {
            summary.last_route_rejection_code = Some(code.clone());
            summary.last_route_rejection_message = Some(message.clone());
            summary.last_route_rejection_at = Some(current_timestamp());
            let rejected_record = route_readiness_from_persisted(&summary, &steps)?;
            let (next_summary, next_steps) =
                persisted_route_readiness_state_from_record(&rejected_record, &summary.updated_at)?;
            self.repository
                .persist_onboarding_route_readiness_state(&next_summary, &next_steps)
                .await
                .map_err(|error| RouteReadinessError::Persistence {
                    message: error.to_string(),
                })?;
            return Err(RouteReadinessError::ActionRejected {
                code: code.clone(),
                message: message.clone(),
            });
        }
        mutation_result?;

        summary.last_route_rejection_code = None;
        summary.last_route_rejection_message = None;
        summary.last_route_rejection_at = None;
        let next_record = route_readiness_from_persisted(&summary, &steps)?;
        let (next_summary, next_steps) =
            persisted_route_readiness_state_from_record(&next_record, &summary.updated_at)?;
        self.repository
            .persist_onboarding_route_readiness_state(&next_summary, &next_steps)
            .await
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?;

        self.inspect_route_readiness(install_id, proposal_id, route_id)
            .await
            .map(RouteReadinessRecord::into_action_result)
    }

    pub async fn materialize_strategy_selection(
        &self,
        request: MaterializeStrategySelectionRequest,
    ) -> Result<StrategySelectionRecord, StrategySelectionError> {
        if let Some(existing) = self
            .repository
            .load_strategy_selection(&request.install_id, &request.proposal_id)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?
        {
            return strategy_selection_record_from_persisted(existing);
        }

        let now = current_timestamp();
        let summary = strategy_selection_summary_from_proposal(
            &request.install_id,
            &request.proposal_id,
            &request.proposal_uri,
            request.proposal_revision,
            &request.proposal,
            &now,
        )?;
        let persisted = persisted_strategy_selection_from_record(&summary)?;
        self.repository
            .persist_strategy_selection_materialized(&persisted)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?;
        Ok(summary)
    }

    pub async fn apply_strategy_selection_override(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
        override_record: ApplyStrategySelectionOverride,
    ) -> Result<StrategySelectionInspection, StrategySelectionError> {
        let current = self
            .repository
            .load_strategy_selection(install_id, proposal_id)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| StrategySelectionError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
            })?;
        if current.selection_id != selection_id {
            return Err(StrategySelectionError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
            });
        }

        let mut record = strategy_selection_record_from_persisted(current.clone())?;
        let sensitivity = sensitivity_for_override_key(
            &record.readiness_sensitivity_summary,
            &override_record.key,
        )
        .ok_or_else(|| StrategySelectionError::InvalidOverrideKey {
            override_key: override_record.key.clone(),
        })?;
        let previous_value = current_override_value(&record, &override_record.key);
        if previous_value == override_record.value {
            return self
                .inspect_strategy_selection(install_id, proposal_id)
                .await;
        }

        record.selection_revision = record.selection_revision.saturating_add(1);
        record.updated_at = current_timestamp();
        update_override_value(
            &mut record,
            &override_record.key,
            override_record.value.clone(),
        );
        let provenance = override_record
            .provenance
            .unwrap_or_else(|| serde_json::json!({ "source": "owner_override" }));
        let approval_history_event = if sensitivity
            == StrategySelectionSensitivityClass::ReadinessSensitive
            && record.approval.approved_revision.is_some()
        {
            record.status = StrategySelectionStatus::Reopened;
            record.reopened_from_revision = record.approval.approved_revision;
            record.approval_stale = true;
            record.approval_stale_reason = Some("readiness_sensitive_override".to_owned());
            Some(PersistedStrategySelectionApprovalHistoryEvent {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
                event_kind: "reopened".to_owned(),
                selection_revision: record.selection_revision,
                approved_revision: record.approval.approved_revision,
                reopened_from_revision: record.approval.approved_revision,
                approved_by: None,
                note: None,
                reason: Some("readiness_sensitive_override".to_owned()),
                provenance: Some(provenance.clone()),
                created_at: record.updated_at.clone(),
            })
        } else {
            None
        };
        let persisted = persisted_strategy_selection_from_record(&record)?;
        let persisted_override = PersistedStrategySelectionOverride {
            install_id: install_id.to_owned(),
            proposal_id: proposal_id.to_owned(),
            selection_id: selection_id.to_owned(),
            selection_revision: record.selection_revision,
            override_key: override_record.key.clone(),
            previous_value,
            new_value: override_record.value,
            rationale: override_record.rationale,
            provenance,
            sensitivity_class: sensitivity.as_str().to_owned(),
            created_at: record.updated_at.clone(),
        };
        self.repository
            .persist_strategy_selection_override(
                &persisted,
                &persisted_override,
                approval_history_event.as_ref(),
            )
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?;
        if sensitivity == StrategySelectionSensitivityClass::ReadinessSensitive {
            self.invalidate_readiness_sensitive_runtime_state(install_id, proposal_id)
                .await
                .map_err(|error| StrategySelectionError::Persistence {
                    message: error.to_string(),
                })?;
        }
        self.inspect_strategy_selection(install_id, proposal_id)
            .await
    }

    pub async fn reopen_strategy_selection(
        &self,
        request: ReopenStrategySelectionRequest,
    ) -> Result<StrategySelectionRecord, StrategySelectionError> {
        let current = self
            .repository
            .load_strategy_selection(&request.install_id, &request.proposal_id)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| StrategySelectionError::NotFound {
                install_id: request.install_id.clone(),
                proposal_id: request.proposal_id.clone(),
            })?;
        if current.selection_id != request.selection_id {
            return Err(StrategySelectionError::NotFound {
                install_id: request.install_id,
                proposal_id: request.proposal_id,
            });
        }

        let mut record = strategy_selection_record_from_persisted(current)?;
        if record.status == StrategySelectionStatus::Reopened && record.approval_stale {
            return Ok(record);
        }

        let Some(approved_revision) = record.approval.approved_revision else {
            return Ok(record);
        };

        record.selection_revision = record.selection_revision.saturating_add(1);
        record.status = StrategySelectionStatus::Reopened;
        record.reopened_from_revision = Some(approved_revision);
        record.approval_stale = true;
        record.approval_stale_reason = Some("operator_requested_reopen".to_owned());
        record.updated_at = current_timestamp();

        let persisted = persisted_strategy_selection_from_record(&record)?;
        let approval_history_event = PersistedStrategySelectionApprovalHistoryEvent {
            install_id: record.install_id.clone(),
            proposal_id: record.proposal_id.clone(),
            selection_id: record.selection_id.clone(),
            event_kind: "reopened".to_owned(),
            selection_revision: record.selection_revision,
            approved_revision: Some(approved_revision),
            reopened_from_revision: Some(approved_revision),
            approved_by: None,
            note: None,
            reason: Some("operator_requested_reopen".to_owned()),
            provenance: Some(serde_json::json!({
                "source": "operator_reopen",
                "reason": request.reason,
            })),
            created_at: record.updated_at.clone(),
        };
        self.repository
            .persist_strategy_selection_approval(&persisted, &approval_history_event)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?;
        Ok(record)
    }

    pub async fn approve_strategy_selection(
        &self,
        request: ApproveStrategySelectionRequest,
    ) -> Result<StrategySelectionRecord, StrategySelectionError> {
        let current = self
            .repository
            .load_strategy_selection(&request.install_id, &request.proposal_id)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| StrategySelectionError::NotFound {
                install_id: request.install_id.clone(),
                proposal_id: request.proposal_id.clone(),
            })?;
        if current.selection_id != request.selection_id {
            return Err(StrategySelectionError::NotFound {
                install_id: request.install_id,
                proposal_id: request.proposal_id,
            });
        }
        if current.selection_revision != request.expected_selection_revision {
            return Err(StrategySelectionError::StaleSelectionRevision {
                expected_selection_revision: request.expected_selection_revision,
                actual_selection_revision: current.selection_revision,
            });
        }

        let mut record = strategy_selection_record_from_persisted(current)?;
        if record.status == StrategySelectionStatus::Approved
            && record.approval.approved_revision == Some(record.selection_revision)
        {
            return Ok(record);
        }

        record.status = StrategySelectionStatus::Approved;
        record.updated_at = current_timestamp();
        record.reopened_from_revision = None;
        record.approval_stale = false;
        record.approval_stale_reason = None;
        let approved_by = request.approval.approved_by;
        let note = request.approval.note;
        record.approval = StrategySelectionApprovalState {
            status: "approved".to_owned(),
            approved_revision: Some(record.selection_revision),
            approved_by: Some(approved_by.clone()),
            note: note.clone(),
            approved_at: Some(record.updated_at.clone()),
        };
        let persisted = persisted_strategy_selection_from_record(&record)?;
        let approval_history_event = PersistedStrategySelectionApprovalHistoryEvent {
            install_id: record.install_id.clone(),
            proposal_id: record.proposal_id.clone(),
            selection_id: record.selection_id.clone(),
            event_kind: "approved".to_owned(),
            selection_revision: record.selection_revision,
            approved_revision: Some(record.selection_revision),
            reopened_from_revision: None,
            approved_by: Some(approved_by),
            note,
            reason: None,
            provenance: None,
            created_at: record.updated_at.clone(),
        };
        self.repository
            .persist_strategy_selection_approval(&persisted, &approval_history_event)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?;
        self.persist_strategy_runtime_handoff(&record)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?;
        Ok(record)
    }

    pub async fn inspect_strategy_selection(
        &self,
        install_id: &str,
        proposal_id: &str,
    ) -> Result<StrategySelectionInspection, StrategySelectionError> {
        let summary = self
            .repository
            .load_strategy_selection(install_id, proposal_id)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| StrategySelectionError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
            })?;
        let overrides = self
            .repository
            .load_strategy_selection_overrides(install_id, proposal_id, &summary.selection_id)
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?;
        let approval_history = self
            .repository
            .load_strategy_selection_approval_history(
                install_id,
                proposal_id,
                &summary.selection_id,
            )
            .await
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?;
        let summary = strategy_selection_record_from_persisted(summary)?;
        let overrides = overrides
            .into_iter()
            .map(strategy_selection_override_from_persisted)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StrategySelectionInspection {
            effective_diff: strategy_selection_effective_diff(&summary, &overrides),
            discussion: StrategySelectionDiscussion {
                recommendation_basis: summary.recommendation_basis.clone(),
            },
            approval_history: approval_history
                .into_iter()
                .map(strategy_selection_approval_history_from_persisted)
                .collect(),
            summary,
            overrides,
        })
    }

    pub async fn inspect_strategy_runtime_eligibility(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<StrategyRuntimeHandoffRecord, StrategyRuntimeHandoffError> {
        self.derive_strategy_runtime_handoff(install_id, proposal_id, selection_id)
            .await
    }

    pub async fn inspect_strategy_operator_report(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<StrategyOperatorReport, StrategyRuntimeHandoffError> {
        let truth = self
            .load_strategy_runtime_truth(install_id, proposal_id, selection_id)
            .await?;
        Ok(strategy_operator_report_from_truth(&truth))
    }

    pub async fn inspect_strategy_exception_rollup(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<StrategyExceptionRollup, StrategyRuntimeHandoffError> {
        let truth = self
            .load_strategy_runtime_truth(install_id, proposal_id, selection_id)
            .await?;
        Ok(strategy_exception_rollup_from_truth(&truth))
    }

    pub async fn inspect_strategy_report_window(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
        cursor: &str,
        window_limit: u32,
    ) -> Result<StrategyReportWindow, StrategyRuntimeHandoffError> {
        let truth = self
            .load_strategy_runtime_truth(install_id, proposal_id, selection_id)
            .await?;
        let current_operator_report = strategy_operator_report_from_truth(&truth);
        let exception_rollup = strategy_exception_rollup_from_truth(&truth);
        let recent_changes = self
            .load_strategy_report_window_changes(
                install_id,
                proposal_id,
                selection_id,
                &truth,
                cursor,
                window_limit,
            )
            .await?;
        let window_start_cursor = recent_changes
            .first()
            .map(|change| change.cursor.clone())
            .unwrap_or_else(|| cursor.to_owned());
        let window_end_cursor = recent_changes
            .last()
            .map(|change| change.cursor.clone())
            .unwrap_or_else(|| cursor.to_owned());

        Ok(StrategyReportWindow {
            report_kind: STRATEGY_REPORT_WINDOW_KIND.to_owned(),
            identity: StrategyReportWindowIdentity {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
            },
            cursor: cursor.to_owned(),
            window_start_cursor,
            window_end_cursor,
            window_limit: normalized_report_window_limit(window_limit),
            freshness: current_operator_report.freshness.clone(),
            current_operator_report,
            recent_changes,
            owner_action_needed_now: exception_rollup.owner_action_needed_now,
            exception_rollup,
        })
    }

    pub async fn inspect_strategy_runtime_monitoring(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<StrategyRuntimeMonitoringSummary, StrategyRuntimeHandoffError> {
        let truth = self
            .load_strategy_runtime_truth(install_id, proposal_id, selection_id)
            .await?;
        let report = strategy_operator_report_from_truth(&truth);
        Ok(StrategyRuntimeMonitoringSummary {
            handoff: truth.handoff,
            report_kind: report.report_kind,
            freshness: report.freshness,
            phase: report.phase,
            current_phase: report.phase,
            runtime_control_gate_status: truth.runtime_control_gate_status,
            last_action: report.last_action,
            next_intended_action: report.next_intended_action,
            control_mode: report.control_mode,
            runtime_control: truth.runtime_control,
            reconciliation_evidence: report.reconciliation_evidence,
            owner_action_needed: report.owner_action_needed,
            recommended_operator_action: report.recommended_operator_action,
            last_runtime_failure: report.last_runtime_failure,
            last_runtime_rejection: report.last_runtime_rejection,
            last_operator_guidance: truth.last_operator_guidance,
        })
    }

    async fn persist_strategy_runtime_handoff(
        &self,
        selection: &StrategySelectionRecord,
    ) -> Result<(), StrategyRuntimeHandoffError> {
        let existing = self
            .repository
            .load_strategy_runtime_handoff(
                &selection.install_id,
                &selection.proposal_id,
                &selection.selection_id,
            )
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;
        let Some(route_state) = self
            .select_strategy_runtime_route(
                &selection.install_id,
                &selection.proposal_id,
                existing.as_ref().map(|handoff| handoff.route_id.as_str()),
            )
            .await?
        else {
            return Ok(());
        };
        let runtime_control = load_runtime_control_record(&self.repository)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;
        let next = build_strategy_runtime_handoff_record(
            selection,
            existing.as_ref(),
            &route_state,
            &runtime_control,
            &current_timestamp(),
        )?;
        self.repository
            .persist_strategy_runtime_handoff(
                &persisted_strategy_runtime_handoff_from_record(&next),
                true,
                true,
            )
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })
    }

    async fn invalidate_readiness_sensitive_runtime_state(
        &self,
        install_id: &str,
        proposal_id: &str,
    ) -> Result<(), StrategyRuntimeHandoffError> {
        let route_states = self
            .repository
            .load_onboarding_route_readiness_states_for_proposal(install_id, proposal_id)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;
        let now = current_timestamp();
        for (summary, steps) in route_states {
            let record = route_readiness_from_persisted(&summary, &steps).map_err(|error| {
                StrategyRuntimeHandoffError::Persistence {
                    message: error.to_string(),
                }
            })?;
            let stale_record = invalidate_route_readiness_record(record, &now);
            let (persisted, persisted_steps) =
                persisted_route_readiness_state_from_record(&stale_record, &now).map_err(
                    |error| StrategyRuntimeHandoffError::Persistence {
                        message: error.to_string(),
                    },
                )?;
            self.repository
                .persist_onboarding_route_readiness_state(&persisted, &persisted_steps)
                .await
                .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                    message: error.to_string(),
                })?;
        }

        Ok(())
    }

    async fn select_strategy_runtime_route(
        &self,
        install_id: &str,
        proposal_id: &str,
        preferred_route_id: Option<&str>,
    ) -> Result<Option<RouteReadinessRecord>, StrategyRuntimeHandoffError> {
        let route_states = self
            .repository
            .load_onboarding_route_readiness_states_for_proposal(install_id, proposal_id)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;

        let mut preferred = None;
        let mut ready = None;
        let mut fallback = None;
        for (summary, steps) in route_states {
            let record = route_readiness_from_persisted(&summary, &steps).map_err(|error| {
                StrategyRuntimeHandoffError::Persistence {
                    message: error.to_string(),
                }
            })?;
            if preferred_route_id == Some(record.identity.route_id.as_str()) {
                preferred = Some(record.clone());
            }
            if ready.is_none()
                && record.status == RouteReadinessStatus::Ready
                && record.current_step_key.is_none()
                && record.stale.as_ref().map(|stale| stale.status)
                    == Some(RouteReadinessStaleStatus::Fresh)
            {
                ready = Some(record.clone());
            }
            if fallback.is_none() {
                fallback = Some(record);
            }
        }
        Ok(preferred.or(ready).or(fallback))
    }

    async fn derive_strategy_runtime_handoff(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<StrategyRuntimeHandoffRecord, StrategyRuntimeHandoffError> {
        let selection = self
            .repository
            .load_strategy_selection(install_id, proposal_id)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| StrategyRuntimeHandoffError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
            })?;
        if selection.selection_id != selection_id {
            return Err(StrategyRuntimeHandoffError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
            });
        }
        let selection = strategy_selection_record_from_persisted(selection).map_err(|error| {
            StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            }
        })?;
        let existing = self
            .repository
            .load_strategy_runtime_handoff(install_id, proposal_id, selection_id)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| StrategyRuntimeHandoffError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
            })?;
        let route_state = self
            .select_strategy_runtime_route(install_id, proposal_id, Some(&existing.route_id))
            .await?
            .ok_or_else(|| StrategyRuntimeHandoffError::Persistence {
                message: format!(
                    "missing canonical route readiness for install {install_id} proposal {proposal_id}"
                ),
            })?;
        let runtime_control = load_runtime_control_record(&self.repository)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;
        let timestamp = current_timestamp();
        let mut handoff = build_strategy_runtime_handoff_record(
            &selection,
            Some(&existing),
            &route_state,
            &runtime_control,
            &timestamp,
        )?;
        if handoff.strategy_id.is_none()
            && let Some(strategy_id) = self.resolve_runtime_strategy_identity().await?
        {
            handoff.strategy_id = Some(strategy_id);
            handoff.runtime_identity_refreshed_at = Some(timestamp.clone());
            handoff.runtime_identity_source = Some("strategy_runtime".to_owned());
            self.repository
                .persist_strategy_runtime_handoff(
                    &persisted_strategy_runtime_handoff_from_record(&handoff),
                    false,
                    false,
                )
                .await
                .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                    message: error.to_string(),
                })?;
        }
        Ok(handoff)
    }

    async fn resolve_runtime_strategy_identity(
        &self,
    ) -> Result<Option<String>, StrategyRuntimeHandoffError> {
        let strategy_ids = self
            .repository
            .load_runtime_strategy_ids()
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;
        if strategy_ids.len() != 1 {
            return Ok(None);
        }
        let strategy_id = strategy_ids[0].clone();
        let has_runtime = self
            .repository
            .load_strategy_recovery_snapshot(&strategy_id)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?
            .is_some();
        Ok(has_runtime.then_some(strategy_id))
    }

    async fn load_strategy_runtime_truth(
        &self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> Result<StrategyRuntimeReportTruth, StrategyRuntimeHandoffError> {
        let mut handoff = self
            .derive_strategy_runtime_handoff(install_id, proposal_id, selection_id)
            .await?;
        let selection = self
            .repository
            .load_strategy_selection(install_id, proposal_id)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| StrategyRuntimeHandoffError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
            })?;
        if selection.selection_id != selection_id {
            return Err(StrategyRuntimeHandoffError::NotFound {
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
            });
        }
        let runtime_control = load_runtime_control_snapshot(&self.repository)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;
        handoff.hold_reason = strategy_runtime_monitoring_hold_reason(
            selection.approval_stale,
            handoff.hold_reason,
            &runtime_control,
        );
        handoff.eligibility_status = if handoff.hold_reason.is_some() {
            StrategyRuntimeEligibilityStatus::Blocked
        } else {
            StrategyRuntimeEligibilityStatus::Eligible
        };
        let recovery = if let Some(strategy_id) = handoff.strategy_id.as_deref() {
            self.repository
                .load_strategy_recovery_snapshot(strategy_id)
                .await
                .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                    message: error.to_string(),
                })?
        } else {
            None
        };
        let runtime_projection = project_strategy_runtime_monitoring(recovery.as_ref());
        let phase = strategy_runtime_phase(&handoff, &runtime_projection.current_phase);
        let last_action = strategy_runtime_action_view(runtime_projection.last_action.clone());
        let next_intended_action = strategy_runtime_next_action(&handoff, &runtime_projection);
        let last_runtime_failure =
            strategy_runtime_last_outcome(runtime_projection.last_runtime_failure);
        let last_runtime_rejection = runtime_control_last_outcome(&runtime_control);
        let last_operator_guidance = strategy_runtime_operator_guidance(&handoff);
        let reconciliation_evidence =
            strategy_runtime_reconciliation_evidence(&self.repository, recovery.as_ref())
                .await
                .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                    message: error.to_string(),
                })?;

        Ok(StrategyRuntimeReportTruth {
            handoff,
            phase,
            runtime_control_gate_status: strategy_runtime_control_gate_status(&runtime_control),
            last_action,
            next_intended_action,
            runtime_control,
            last_runtime_failure,
            last_runtime_rejection,
            last_operator_guidance,
            reconciliation_evidence,
            reported_at: current_timestamp(),
        })
    }

    async fn load_strategy_report_window_changes(
        &self,
        _install_id: &str,
        proposal_id: &str,
        selection_id: &str,
        truth: &StrategyRuntimeReportTruth,
        cursor: &str,
        window_limit: u32,
    ) -> Result<Vec<StrategyReportWindowChange>, StrategyRuntimeHandoffError> {
        let mut stream_filters = vec![
            ("strategy_selection".to_owned(), proposal_id.to_owned()),
            ("strategy_selection".to_owned(), selection_id.to_owned()),
            (
                "strategy_runtime_handoff".to_owned(),
                selection_id.to_owned(),
            ),
            (
                "runtime_control".to_owned(),
                AUTONOMOUS_RUNTIME_CONTROL_SCOPE.to_owned(),
            ),
        ];
        if let Some(strategy_id) = truth.handoff.strategy_id.clone() {
            stream_filters.push(("strategy".to_owned(), strategy_id));
        }
        if let Some(execution_id) = truth
            .reconciliation_evidence
            .execution_id
            .clone()
            .filter(|value| !value.is_empty())
        {
            stream_filters.push(("execution".to_owned(), execution_id.clone()));
            stream_filters.push(("reconciliation".to_owned(), execution_id));
        }

        let journal_limit = (normalized_report_window_limit(window_limit) as usize)
            .saturating_mul(8)
            .max(64);
        let journal = self
            .repository
            .load_journal_window_for_streams(&stream_filters, journal_limit)
            .await
            .map_err(|error| StrategyRuntimeHandoffError::Persistence {
                message: error.to_string(),
            })?;
        let cursor_floor = parse_report_cursor(cursor);

        let mut changes = journal
            .into_iter()
            .filter(|entry| report_cursor_is_after(entry, cursor_floor.as_ref()))
            .filter_map(strategy_report_window_change_from_journal_entry)
            .collect::<Vec<_>>();
        changes.sort_by(|left, right| {
            left.observed_at
                .cmp(&right.observed_at)
                .then_with(|| left.cursor.cmp(&right.cursor))
        });
        changes.truncate(normalized_report_window_limit(window_limit) as usize);
        Ok(changes)
    }

    async fn load_guided_install_state(
        &self,
        install_id: &str,
    ) -> Result<
        (
            a2ex_state::PersistedOnboardingInstall,
            Vec<PersistedOnboardingChecklistItem>,
        ),
        GuidedOnboardingError,
    > {
        self.repository
            .load_onboarding_install_state(install_id)
            .await
            .map_err(|error| GuidedOnboardingError::Persistence {
                message: error.to_string(),
            })?
            .ok_or_else(|| GuidedOnboardingError::Persistence {
                message: format!("missing persisted install {install_id}"),
            })
    }
}

#[derive(Debug, Clone)]
struct StrategyRuntimeReportTruth {
    handoff: StrategyRuntimeHandoffRecord,
    phase: StrategyRuntimePhase,
    runtime_control_gate_status: StrategyRuntimeControlGateStatus,
    last_action: Option<StrategyRuntimeActionView>,
    next_intended_action: Option<StrategyRuntimeActionView>,
    runtime_control: RuntimeControlSnapshot,
    last_runtime_failure: Option<StrategyRuntimeLastOutcome>,
    last_runtime_rejection: Option<StrategyRuntimeLastOutcome>,
    last_operator_guidance: Option<StrategyRuntimeOperatorGuidance>,
    reconciliation_evidence: StrategyOperatorReconciliationEvidence,
    reported_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReportCursor {
    observed_at: String,
    event_id: String,
}

fn normalized_report_window_limit(window_limit: u32) -> u32 {
    window_limit.clamp(1, 100)
}

fn parse_report_cursor(cursor: &str) -> Option<ReportCursor> {
    if cursor == "bootstrap" {
        return None;
    }
    let (observed_at, event_id) = cursor.split_once('|')?;
    Some(ReportCursor {
        observed_at: observed_at.to_owned(),
        event_id: event_id.to_owned(),
    })
}

fn report_cursor_for_entry(entry: &JournalEntry) -> ReportCursor {
    ReportCursor {
        observed_at: entry.created_at.clone(),
        event_id: entry.event_id.clone(),
    }
}

fn report_cursor_string(entry: &JournalEntry) -> String {
    let cursor = report_cursor_for_entry(entry);
    format!("{}|{}", cursor.observed_at, cursor.event_id)
}

fn report_cursor_is_after(entry: &JournalEntry, cursor_floor: Option<&ReportCursor>) -> bool {
    cursor_floor
        .map(|floor| report_cursor_for_entry(entry) > *floor)
        .unwrap_or(true)
}

fn strategy_exception_rollup_from_truth(
    truth: &StrategyRuntimeReportTruth,
) -> StrategyExceptionRollup {
    let operator_report = strategy_operator_report_from_truth(truth);
    let active_hold = truth
        .handoff
        .hold_reason
        .map(|reason_code| StrategyRuntimeHoldException {
            reason_code,
            summary: strategy_hold_summary(reason_code),
            observed_at: Some(truth.runtime_control.updated_at.clone()),
        });

    StrategyExceptionRollup {
        report_kind: STRATEGY_EXCEPTION_ROLLUP_KIND.to_owned(),
        identity: StrategyReportWindowIdentity {
            install_id: truth.handoff.install_id.clone(),
            proposal_id: truth.handoff.proposal_id.clone(),
            selection_id: truth.handoff.selection_id.clone(),
        },
        owner_action_needed_now: operator_report.owner_action_needed,
        urgency: strategy_exception_urgency(truth),
        recommended_operator_action: operator_report.recommended_operator_action.clone(),
        active_hold,
        last_runtime_failure: truth.last_runtime_failure.clone(),
        last_runtime_rejection: truth.last_runtime_rejection.clone(),
    }
}

fn strategy_exception_urgency(truth: &StrategyRuntimeReportTruth) -> StrategyExceptionUrgency {
    if truth.handoff.hold_reason.is_some()
        || truth.last_runtime_failure.is_some()
        || truth.last_runtime_rejection.is_some()
    {
        StrategyExceptionUrgency::ActionRequired
    } else if truth.reconciliation_evidence.rebalance_required {
        StrategyExceptionUrgency::InvestigateSoon
    } else {
        StrategyExceptionUrgency::Monitor
    }
}

fn strategy_hold_summary(reason: StrategyRuntimeHoldReason) -> String {
    match reason {
        StrategyRuntimeHoldReason::RouteReadinessStale => {
            "Route readiness is stale and must be refreshed before autonomous execution can continue."
                .to_owned()
        }
        StrategyRuntimeHoldReason::RouteReadinessIncomplete => {
            "Route readiness still has incomplete operator steps, so the runtime remains on hold."
                .to_owned()
        }
        StrategyRuntimeHoldReason::RouteReadinessMissing => {
            "No canonical route readiness evaluation is available for the approved runtime handoff."
                .to_owned()
        }
        StrategyRuntimeHoldReason::ApprovedSelectionRevisionStale => {
            "The approved selection revision is stale and needs a new owner review before the runtime can proceed."
                .to_owned()
        }
        StrategyRuntimeHoldReason::RuntimeControlPaused => {
            "Runtime control is paused, so autonomous actions are intentionally held pending operator intervention."
                .to_owned()
        }
        StrategyRuntimeHoldReason::RuntimeControlStopped => {
            "Runtime control is stopped and must be cleared before autonomous actions can resume."
                .to_owned()
        }
    }
}

fn strategy_report_window_change_from_journal_entry(
    entry: JournalEntry,
) -> Option<StrategyReportWindowChange> {
    let payload = serde_json::from_str::<serde_json::Value>(&entry.payload_json).ok();
    let change_kind = match entry.event_type.as_str() {
        "strategy_selection_materialized" => StrategyReportWindowChangeKind::SelectionMaterialized,
        "strategy_selection_override_applied" => {
            StrategyReportWindowChangeKind::SelectionOverrideApplied
        }
        "strategy_selection_approved" => StrategyReportWindowChangeKind::SelectionApproved,
        "strategy_selection_reopened" => StrategyReportWindowChangeKind::SelectionReopened,
        "strategy_runtime_handoff_persisted" => {
            StrategyReportWindowChangeKind::RuntimeHandoffPersisted
        }
        "strategy_runtime_eligibility_changed" => {
            StrategyReportWindowChangeKind::RuntimeEligibilityChanged
        }
        "strategy_runtime_identity_refreshed" => {
            StrategyReportWindowChangeKind::RuntimeIdentityRefreshed
        }
        "runtime_control_changed" => StrategyReportWindowChangeKind::RuntimeControlChanged,
        "strategy_state_changed" => StrategyReportWindowChangeKind::RuntimeStateChanged,
        "execution_state_changed" => StrategyReportWindowChangeKind::ExecutionStateChanged,
        "reconciliation_state_changed" => {
            StrategyReportWindowChangeKind::ReconciliationStateChanged
        }
        _ => StrategyReportWindowChangeKind::JournalEvent,
    };
    if change_kind == StrategyReportWindowChangeKind::JournalEvent {
        return None;
    }

    let (summary, operator_impact) = strategy_report_window_summary(&change_kind, payload.as_ref());

    Some(StrategyReportWindowChange {
        cursor: report_cursor_string(&entry),
        change_kind,
        observed_at: entry.created_at,
        summary,
        operator_impact,
    })
}

fn strategy_report_window_summary(
    kind: &StrategyReportWindowChangeKind,
    payload: Option<&serde_json::Value>,
) -> (String, String) {
    match kind {
        StrategyReportWindowChangeKind::SelectionMaterialized => (
            format!(
                "Selection {} materialized from canonical proposal state.",
                payload
                    .and_then(|value| value.get("selection_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("state")
            ),
            "A canonical selection baseline now exists for reconnect-safe report rereads."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::SelectionOverrideApplied => (
            format!(
                "Owner override `{}` changed the approved selection.",
                payload
                    .and_then(|value| value.get("override_key"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            "Review readiness and approval state because the runtime inputs changed."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::SelectionApproved => (
            format!(
                "Selection revision {} was approved for runtime handoff.",
                payload
                    .and_then(|value| value.get("selection_revision"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default()
            ),
            "The runtime can proceed once route readiness is fresh and control gates remain open."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::SelectionReopened => (
            "The approved selection was reopened after a readiness-sensitive change.".to_owned(),
            "Owner review is required before autonomous execution can continue."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::RuntimeHandoffPersisted => (
            format!(
                "Canonical runtime handoff now targets route {}.",
                payload
                    .and_then(|value| value.get("route_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            "The report window is now anchored to the latest approved route-readiness evidence."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::RuntimeEligibilityChanged => {
            let hold_reason = payload
                .and_then(|value| value.get("hold_reason"))
                .and_then(serde_json::Value::as_str);
            if let Some(hold_reason) = hold_reason {
                (
                    format!("Runtime eligibility changed and is now blocked by {hold_reason}."),
                    "Inspect the hold separately from failures and control rejections before resuming automation."
                        .to_owned(),
                )
            } else {
                (
                    "Runtime eligibility changed and remains clear for autonomous execution."
                        .to_owned(),
                    "The runtime is unblocked if no newer hold, failure, or rejection appears."
                        .to_owned(),
                )
            }
        }
        StrategyReportWindowChangeKind::RuntimeIdentityRefreshed => (
            format!(
                "Runtime identity refreshed to strategy {}.",
                payload
                    .and_then(|value| value.get("strategy_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            "Subsequent runtime, execution, and reconciliation evidence now ties to a canonical strategy id."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::RuntimeControlChanged => (
            format!(
                "Runtime control changed to {}.",
                payload
                    .and_then(|value| value.get("control_mode"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            "Control-gate transitions are separate from runtime execution failures and remain operator-actionable after reconnect."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::RuntimeStateChanged => (
            format!(
                "Strategy runtime state changed to {}.",
                payload
                    .and_then(|value| value.get("runtime_state"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            "Canonical runtime progress moved forward; reread the embedded operator report for the current phase."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::ExecutionStateChanged => (
            format!(
                "Execution {} changed state to {}.",
                payload
                    .and_then(|value| value.get("execution_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown"),
                payload
                    .and_then(|value| value.get("status"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            "Execution progress or failure changed and should be compared against the exception rollup."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::ReconciliationStateChanged => (
            format!(
                "Reconciliation for execution {} recorded a new residual exposure check.",
                payload
                    .and_then(|value| value.get("execution_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            "Reconciliation evidence may require rebalancing even when the runtime control gate remains open."
                .to_owned(),
        ),
        StrategyReportWindowChangeKind::JournalEvent => (
            "Canonical journal evidence changed.".to_owned(),
            "Inspect the updated operator report for the latest actionable state.".to_owned(),
        ),
    }
}

fn strategy_operator_report_from_truth(
    truth: &StrategyRuntimeReportTruth,
) -> StrategyOperatorReport {
    let recommended_operator_action = strategy_operator_recommended_action(truth);
    StrategyOperatorReport {
        report_kind: STRATEGY_OPERATOR_REPORT_KIND.to_owned(),
        freshness: StrategyOperatorReportFreshness {
            reported_at: truth.reported_at.clone(),
            route_readiness_evaluated_at: truth.handoff.route_readiness_evaluated_at.clone(),
            runtime_control_updated_at: truth.runtime_control.updated_at.clone(),
            runtime_identity_refreshed_at: truth.handoff.runtime_identity_refreshed_at.clone(),
            reconciliation_observed_at: truth.reconciliation_evidence.observed_at.clone(),
        },
        phase: truth.phase,
        last_action: truth.last_action.clone(),
        next_intended_action: truth.next_intended_action.clone(),
        hold_reason: truth.handoff.hold_reason,
        control_mode: truth.runtime_control.control_mode.clone(),
        reconciliation_evidence: truth.reconciliation_evidence.clone(),
        owner_action_needed: strategy_operator_owner_action_needed(truth),
        recommended_operator_action,
        last_runtime_failure: truth.last_runtime_failure.clone(),
        last_runtime_rejection: truth.last_runtime_rejection.clone(),
    }
}

async fn strategy_runtime_reconciliation_evidence(
    repository: &StateRepository,
    recovery: Option<&a2ex_state::PersistedStrategyRecoverySnapshot>,
) -> Result<StrategyOperatorReconciliationEvidence, a2ex_state::StateError> {
    let execution_id = recovery
        .and_then(|recovery| recovery.pending_hedge.as_ref())
        .map(|hedge| hedge.client_order_id.clone());
    let Some(execution_id) = execution_id else {
        return Ok(StrategyOperatorReconciliationEvidence {
            status: StrategyOperatorReconciliationStatus::NotStarted,
            summary:
                "No canonical autonomous execution or reconciliation evidence has been recorded for this strategy yet."
                    .to_owned(),
            execution_id: None,
            execution_status: None,
            residual_exposure_usd: None,
            rebalance_required: false,
            observed_at: None,
        });
    };

    let snapshot = repository.load_snapshot().await?;
    let execution = snapshot
        .executions
        .iter()
        .find(|execution| execution.execution_id == execution_id);
    let reconciliation = snapshot
        .reconciliations
        .iter()
        .find(|reconciliation| reconciliation.execution_id == execution_id);
    let projection = project_strategy_runtime_reconciliation(execution, reconciliation);

    Ok(StrategyOperatorReconciliationEvidence {
        status: match projection.status.as_str() {
            "reconciled" => StrategyOperatorReconciliationStatus::Reconciled,
            "rebalance_required" => StrategyOperatorReconciliationStatus::RebalanceRequired,
            "pending" => StrategyOperatorReconciliationStatus::Pending,
            _ => StrategyOperatorReconciliationStatus::NotStarted,
        },
        summary: projection.summary,
        execution_id: projection.execution_id,
        execution_status: projection.execution_status,
        residual_exposure_usd: projection.residual_exposure_usd,
        rebalance_required: projection.rebalance_required,
        observed_at: projection.observed_at,
    })
}

fn strategy_operator_owner_action_needed(truth: &StrategyRuntimeReportTruth) -> bool {
    truth.handoff.hold_reason.is_some()
        || truth.last_runtime_failure.is_some()
        || truth.last_runtime_rejection.is_some()
        || truth.reconciliation_evidence.rebalance_required
}

fn strategy_operator_recommended_action(truth: &StrategyRuntimeReportTruth) -> String {
    if let Some(guidance) = truth.last_operator_guidance.as_ref() {
        return guidance.recommended_action.clone();
    }
    if truth.reconciliation_evidence.rebalance_required {
        return "review_reconciliation_rebalance".to_owned();
    }
    if truth.last_runtime_failure.is_some() {
        return "inspect_runtime_failure".to_owned();
    }
    if truth.last_runtime_rejection.is_some() {
        return "inspect_runtime_control_rejection".to_owned();
    }
    "monitor_runtime".to_owned()
}

fn strategy_selection_summary_from_proposal(
    install_id: &str,
    proposal_id: &str,
    proposal_uri: &str,
    proposal_revision: u64,
    proposal: &SkillProposalPacket,
    timestamp: &str,
) -> Result<StrategySelectionRecord, StrategySelectionError> {
    let selection_id = deterministic_id("selection", &[install_id, proposal_id, proposal_uri]);
    let readiness_sensitive_override_keys = proposal
        .owner_override_points
        .iter()
        .map(|decision| decision.decision_key.clone())
        .collect::<Vec<_>>();
    let proposal_snapshot =
        serde_json::to_value(proposal).map_err(|error| StrategySelectionError::Persistence {
            message: error.to_string(),
        })?;

    Ok(StrategySelectionRecord {
        install_id: install_id.to_owned(),
        proposal_id: proposal_id.to_owned(),
        selection_id,
        selection_revision: 1,
        status: StrategySelectionStatus::Recommended,
        reopened_from_revision: None,
        proposal_revision,
        proposal_uri: proposal_uri.to_owned(),
        proposal_snapshot,
        recommendation_basis: StrategySelectionRecommendationBasis {
            source_kind: "proposal_packet".to_owned(),
            proposal_uri: proposal_uri.to_owned(),
            proposal_revision,
        },
        readiness_sensitivity_summary: StrategySelectionReadinessSensitivitySummary {
            readiness_sensitive_override_keys,
            advisory_override_keys: Vec::new(),
        },
        approval: StrategySelectionApprovalState {
            status: "pending".to_owned(),
            approved_revision: None,
            approved_by: None,
            note: None,
            approved_at: None,
        },
        approval_stale: false,
        approval_stale_reason: None,
        created_at: timestamp.to_owned(),
        updated_at: timestamp.to_owned(),
    })
}

fn persisted_strategy_selection_from_record(
    record: &StrategySelectionRecord,
) -> Result<PersistedStrategySelection, StrategySelectionError> {
    Ok(PersistedStrategySelection {
        install_id: record.install_id.clone(),
        proposal_id: record.proposal_id.clone(),
        selection_id: record.selection_id.clone(),
        selection_revision: record.selection_revision,
        status: record.status.as_str().to_owned(),
        reopened_from_revision: record.reopened_from_revision,
        proposal_revision: record.proposal_revision as i64,
        proposal_uri: record.proposal_uri.clone(),
        proposal_snapshot: record.proposal_snapshot.clone(),
        recommendation_basis: serde_json::to_value(&record.recommendation_basis).map_err(
            |error| StrategySelectionError::Persistence {
                message: error.to_string(),
            },
        )?,
        readiness_sensitivity_summary: serde_json::to_value(&record.readiness_sensitivity_summary)
            .map_err(|error| StrategySelectionError::Persistence {
                message: error.to_string(),
            })?,
        approval: serde_json::to_value(&record.approval).map_err(|error| {
            StrategySelectionError::Persistence {
                message: error.to_string(),
            }
        })?,
        approval_stale: record.approval_stale,
        approval_stale_reason: record.approval_stale_reason.clone(),
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
    })
}

fn strategy_selection_record_from_persisted(
    persisted: PersistedStrategySelection,
) -> Result<StrategySelectionRecord, StrategySelectionError> {
    Ok(StrategySelectionRecord {
        install_id: persisted.install_id,
        proposal_id: persisted.proposal_id,
        selection_id: persisted.selection_id,
        selection_revision: persisted.selection_revision,
        status: StrategySelectionStatus::from_str(&persisted.status).ok_or_else(|| {
            StrategySelectionError::Persistence {
                message: format!("unknown strategy selection status {}", persisted.status),
            }
        })?,
        reopened_from_revision: persisted.reopened_from_revision,
        proposal_revision: persisted.proposal_revision as u64,
        proposal_uri: persisted.proposal_uri,
        proposal_snapshot: persisted.proposal_snapshot,
        recommendation_basis: serde_json::from_value(persisted.recommendation_basis).map_err(
            |error| StrategySelectionError::Persistence {
                message: error.to_string(),
            },
        )?,
        readiness_sensitivity_summary: serde_json::from_value(
            persisted.readiness_sensitivity_summary,
        )
        .map_err(|error| StrategySelectionError::Persistence {
            message: error.to_string(),
        })?,
        approval: serde_json::from_value(persisted.approval).map_err(|error| {
            StrategySelectionError::Persistence {
                message: error.to_string(),
            }
        })?,
        approval_stale: persisted.approval_stale,
        approval_stale_reason: persisted.approval_stale_reason,
        created_at: persisted.created_at,
        updated_at: persisted.updated_at,
    })
}

fn strategy_selection_override_from_persisted(
    persisted: PersistedStrategySelectionOverride,
) -> Result<StrategySelectionOverrideRecord, StrategySelectionError> {
    Ok(StrategySelectionOverrideRecord {
        install_id: persisted.install_id,
        proposal_id: persisted.proposal_id,
        selection_id: persisted.selection_id,
        selection_revision: persisted.selection_revision,
        override_key: persisted.override_key,
        previous_value: persisted.previous_value,
        new_value: persisted.new_value,
        rationale: persisted.rationale,
        provenance: persisted.provenance,
        sensitivity_class: StrategySelectionSensitivityClass::from_str(
            &persisted.sensitivity_class,
        )
        .ok_or_else(|| StrategySelectionError::Persistence {
            message: format!(
                "unknown strategy selection sensitivity class {}",
                persisted.sensitivity_class
            ),
        })?,
        created_at: persisted.created_at,
    })
}

fn strategy_selection_approval_history_from_persisted(
    persisted: PersistedStrategySelectionApprovalHistoryEvent,
) -> StrategySelectionApprovalHistoryEvent {
    StrategySelectionApprovalHistoryEvent {
        event_kind: persisted.event_kind,
        selection_revision: persisted.selection_revision,
        approved_revision: persisted.approved_revision,
        reopened_from_revision: persisted.reopened_from_revision,
        approved_by: persisted.approved_by,
        note: persisted.note,
        reason: persisted.reason,
        provenance: persisted.provenance,
        created_at: persisted.created_at,
    }
}

fn strategy_selection_effective_diff(
    summary: &StrategySelectionRecord,
    overrides: &[StrategySelectionOverrideRecord],
) -> StrategySelectionEffectiveDiff {
    let changed_override_keys = current_changed_override_keys(summary, overrides);
    let readiness_sensitive_changes = changed_override_keys
        .iter()
        .filter(|key| {
            summary
                .readiness_sensitivity_summary
                .readiness_sensitive_override_keys
                .iter()
                .any(|candidate| candidate == *key)
        })
        .cloned()
        .collect::<Vec<_>>();
    let advisory_changes = changed_override_keys
        .iter()
        .filter(|key| {
            summary
                .readiness_sensitivity_summary
                .advisory_override_keys
                .iter()
                .any(|candidate| candidate == *key)
        })
        .cloned()
        .collect::<Vec<_>>();

    StrategySelectionEffectiveDiff {
        baseline_kind: "recommended".to_owned(),
        changed_override_keys,
        readiness_sensitive_changes: readiness_sensitive_changes.clone(),
        advisory_changes,
        readiness_stale: !readiness_sensitive_changes.is_empty(),
        approval_stale: summary.approval_stale,
        approval_stale_reason: summary.approval_stale_reason.clone(),
    }
}

fn current_changed_override_keys(
    _summary: &StrategySelectionRecord,
    overrides: &[StrategySelectionOverrideRecord],
) -> Vec<String> {
    let mut first_previous = BTreeMap::new();
    let mut latest = BTreeMap::new();
    for record in overrides {
        first_previous
            .entry(record.override_key.clone())
            .or_insert_with(|| record.previous_value.clone());
        latest.insert(record.override_key.clone(), record.new_value.clone());
    }
    latest
        .into_iter()
        .filter_map(|(key, value)| {
            (first_previous
                .get(&key)
                .is_some_and(|previous| previous != &value))
            .then_some(key)
        })
        .collect()
}

fn sensitivity_for_override_key(
    summary: &StrategySelectionReadinessSensitivitySummary,
    key: &str,
) -> Option<StrategySelectionSensitivityClass> {
    if summary
        .readiness_sensitive_override_keys
        .iter()
        .any(|candidate| candidate == key)
    {
        return Some(StrategySelectionSensitivityClass::ReadinessSensitive);
    }
    if summary
        .advisory_override_keys
        .iter()
        .any(|candidate| candidate == key)
    {
        return Some(StrategySelectionSensitivityClass::Advisory);
    }
    None
}

fn current_override_value(record: &StrategySelectionRecord, key: &str) -> serde_json::Value {
    record
        .proposal_snapshot
        .get("owner_override_points")
        .and_then(|value| value.as_array())
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("decision_key").and_then(serde_json::Value::as_str) == Some(key)).then(
                    || {
                        item.get("selected_value")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({ "resolution": "pending" }))
                    },
                )
            })
        })
        .unwrap_or_else(|| serde_json::json!({ "resolution": "pending" }))
}

fn update_override_value(
    record: &mut StrategySelectionRecord,
    key: &str,
    value: serde_json::Value,
) {
    if let Some(items) = record
        .proposal_snapshot
        .get_mut("owner_override_points")
        .and_then(serde_json::Value::as_array_mut)
        && let Some(item) = items
            .iter_mut()
            .find(|item| item.get("decision_key").and_then(serde_json::Value::as_str) == Some(key))
    {
        item["selected_value"] = value;
    }
}

async fn load_runtime_control_record(
    repository: &StateRepository,
) -> Result<PersistedRuntimeControl, a2ex_state::StateError> {
    Ok(repository
        .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
        .await?
        .unwrap_or_else(default_runtime_control_record))
}

async fn load_runtime_control_snapshot(
    repository: &StateRepository,
) -> Result<RuntimeControlSnapshot, a2ex_state::StateError> {
    Ok(runtime_control_snapshot(
        &load_runtime_control_record(repository).await?,
    ))
}

fn default_runtime_control_record() -> PersistedRuntimeControl {
    PersistedRuntimeControl {
        scope_key: AUTONOMOUS_RUNTIME_CONTROL_SCOPE.to_owned(),
        control_mode: "active".to_owned(),
        transition_reason: "initial_state".to_owned(),
        transition_source: "daemon".to_owned(),
        transitioned_at: "2026-03-11T00:00:00Z".to_owned(),
        last_cleared_at: None,
        last_cleared_reason: None,
        last_cleared_source: None,
        last_rejection_code: None,
        last_rejection_message: None,
        last_rejection_operation: None,
        last_rejection_at: None,
        updated_at: "2026-03-11T00:00:00Z".to_owned(),
    }
}

fn runtime_control_snapshot(record: &PersistedRuntimeControl) -> RuntimeControlSnapshot {
    RuntimeControlSnapshot {
        control_mode: record.control_mode.clone(),
        transition_reason: record.transition_reason.clone(),
        transition_source: record.transition_source.clone(),
        transitioned_at: record.transitioned_at.clone(),
        last_rejection_code: record.last_rejection_code.clone(),
        last_rejection_message: record.last_rejection_message.clone(),
        last_rejection_operation: record.last_rejection_operation.clone(),
        last_rejection_at: record.last_rejection_at.clone(),
        updated_at: record.updated_at.clone(),
    }
}

fn runtime_control_last_outcome(
    record: &RuntimeControlSnapshot,
) -> Option<StrategyRuntimeLastOutcome> {
    Some(StrategyRuntimeLastOutcome {
        code: record.last_rejection_code.clone()?,
        message: record.last_rejection_message.clone().unwrap_or_default(),
        observed_at: record.last_rejection_at.clone().unwrap_or_default(),
    })
}

fn strategy_runtime_phase(
    handoff: &StrategyRuntimeHandoffRecord,
    projected_phase: &str,
) -> StrategyRuntimePhase {
    if handoff.hold_reason.is_some() {
        return StrategyRuntimePhase::Blocked;
    }

    match projected_phase {
        "idle" => StrategyRuntimePhase::Idle,
        "active" => StrategyRuntimePhase::Active,
        "rebalancing" => StrategyRuntimePhase::Rebalancing,
        "syncing_hedge" => StrategyRuntimePhase::SyncingHedge,
        "recovering" => StrategyRuntimePhase::Recovering,
        "unwinding" => StrategyRuntimePhase::Unwinding,
        _ => StrategyRuntimePhase::AwaitingRuntimeIdentity,
    }
}

fn strategy_runtime_control_gate_status(
    runtime_control: &RuntimeControlSnapshot,
) -> StrategyRuntimeControlGateStatus {
    match runtime_control.control_mode.as_str() {
        "paused" => StrategyRuntimeControlGateStatus::Paused,
        "stopped" => StrategyRuntimeControlGateStatus::Stopped,
        _ => StrategyRuntimeControlGateStatus::Open,
    }
}

fn strategy_runtime_monitoring_hold_reason(
    approval_stale: bool,
    handoff_hold_reason: Option<StrategyRuntimeHoldReason>,
    runtime_control: &RuntimeControlSnapshot,
) -> Option<StrategyRuntimeHoldReason> {
    if approval_stale {
        return Some(StrategyRuntimeHoldReason::ApprovedSelectionRevisionStale);
    }

    match runtime_control.control_mode.as_str() {
        "paused" => Some(StrategyRuntimeHoldReason::RuntimeControlPaused),
        "stopped" => Some(StrategyRuntimeHoldReason::RuntimeControlStopped),
        _ => handoff_hold_reason,
    }
}

fn strategy_runtime_action_view(
    action: Option<a2ex_daemon::StrategyRuntimeActionProjection>,
) -> Option<StrategyRuntimeActionView> {
    action.map(|action| StrategyRuntimeActionView {
        kind: action.kind,
        status: action.status,
        summary: action.summary,
        observed_at: action.observed_at,
    })
}

fn strategy_runtime_last_outcome(
    outcome: Option<a2ex_daemon::StrategyRuntimeOutcomeProjection>,
) -> Option<StrategyRuntimeLastOutcome> {
    outcome.map(|outcome| StrategyRuntimeLastOutcome {
        code: outcome.code,
        message: outcome.message,
        observed_at: outcome.observed_at,
    })
}

fn strategy_runtime_next_action(
    handoff: &StrategyRuntimeHandoffRecord,
    projection: &a2ex_daemon::StrategyRuntimeMonitoringProjection,
) -> Option<StrategyRuntimeActionView> {
    if let Some(hold_reason) = handoff.hold_reason {
        let (kind, summary) = match hold_reason {
            StrategyRuntimeHoldReason::RouteReadinessStale => (
                "refresh_route_readiness",
                "Route readiness is stale; reevaluate canonical readiness before autonomous runtime resumes.",
            ),
            StrategyRuntimeHoldReason::RouteReadinessIncomplete
            | StrategyRuntimeHoldReason::RouteReadinessMissing => (
                "complete_route_readiness",
                "Route readiness is incomplete; satisfy the remaining canonical readiness steps before autonomous runtime resumes.",
            ),
            StrategyRuntimeHoldReason::ApprovedSelectionRevisionStale => (
                "reapprove_selection",
                "The approved selection revision is stale; reapprove the canonical selection before autonomous runtime resumes.",
            ),
            StrategyRuntimeHoldReason::RuntimeControlPaused
            | StrategyRuntimeHoldReason::RuntimeControlStopped => (
                "clear_runtime_control_hold",
                "Runtime control is blocking autonomous actions; clear the canonical pause or stop before autonomous runtime resumes.",
            ),
        };
        return Some(StrategyRuntimeActionView {
            kind: kind.to_owned(),
            status: "blocked".to_owned(),
            summary: summary.to_owned(),
            observed_at: Some(handoff.updated_at.clone()),
        });
    }

    strategy_runtime_action_view(projection.next_intended_action.clone())
}

fn strategy_runtime_operator_guidance(
    handoff: &StrategyRuntimeHandoffRecord,
) -> Option<StrategyRuntimeOperatorGuidance> {
    let hold_reason = handoff.hold_reason?;
    let (recommended_action, summary) = match hold_reason {
        StrategyRuntimeHoldReason::RouteReadinessStale => (
            "refresh_route_readiness",
            "Route readiness is stale; reevaluate canonical readiness before runtime resumes.",
        ),
        StrategyRuntimeHoldReason::RouteReadinessIncomplete
        | StrategyRuntimeHoldReason::RouteReadinessMissing => (
            "complete_route_readiness",
            "Route readiness is incomplete; satisfy the remaining canonical steps before runtime resumes.",
        ),
        StrategyRuntimeHoldReason::ApprovedSelectionRevisionStale => (
            "reapprove_selection",
            "The approved selection revision is stale; reapprove the current canonical selection before runtime resumes.",
        ),
        StrategyRuntimeHoldReason::RuntimeControlPaused
        | StrategyRuntimeHoldReason::RuntimeControlStopped => (
            "clear_runtime_control_hold",
            "Runtime control is blocking autonomous execution; clear the canonical pause or stop before runtime resumes.",
        ),
    };
    Some(StrategyRuntimeOperatorGuidance {
        recommended_action: recommended_action.to_owned(),
        summary: summary.to_owned(),
    })
}

fn build_strategy_runtime_handoff_record(
    selection: &StrategySelectionRecord,
    existing: Option<&PersistedStrategyRuntimeHandoff>,
    route_state: &RouteReadinessRecord,
    runtime_control: &PersistedRuntimeControl,
    timestamp: &str,
) -> Result<StrategyRuntimeHandoffRecord, StrategyRuntimeHandoffError> {
    let route_readiness_fingerprint = route_state
        .evaluation
        .as_ref()
        .map(|evaluation| evaluation.fingerprint.clone())
        .unwrap_or_default();
    let route_readiness_evaluated_at = route_state
        .evaluated_at
        .clone()
        .or_else(|| {
            route_state
                .evaluation
                .as_ref()
                .map(|evaluation| evaluation.evaluated_at.clone())
        })
        .unwrap_or_else(|| timestamp.to_owned());
    let hold_reason = derive_strategy_runtime_hold_reason(selection, route_state, runtime_control);
    let eligibility_status = if hold_reason.is_some() {
        StrategyRuntimeEligibilityStatus::Blocked
    } else {
        StrategyRuntimeEligibilityStatus::Eligible
    };

    Ok(StrategyRuntimeHandoffRecord {
        install_id: selection.install_id.clone(),
        proposal_id: selection.proposal_id.clone(),
        selection_id: selection.selection_id.clone(),
        approved_selection_revision: selection
            .approval
            .approved_revision
            .unwrap_or(selection.selection_revision),
        route_id: route_state.identity.route_id.clone(),
        request_id: route_state.identity.request_id.clone(),
        route_readiness_fingerprint,
        route_readiness_status: route_state.status,
        route_readiness_evaluated_at,
        eligibility_status,
        hold_reason,
        runtime_control_mode: runtime_control.control_mode.clone(),
        strategy_id: existing.and_then(|handoff| handoff.strategy_id.clone()),
        runtime_identity_refreshed_at: existing
            .and_then(|handoff| handoff.runtime_identity_refreshed_at.clone()),
        runtime_identity_source: existing
            .and_then(|handoff| handoff.runtime_identity_source.clone()),
        created_at: existing
            .map(|handoff| handoff.created_at.clone())
            .unwrap_or_else(|| timestamp.to_owned()),
        updated_at: timestamp.to_owned(),
    })
}

fn derive_strategy_runtime_hold_reason(
    selection: &StrategySelectionRecord,
    route_state: &RouteReadinessRecord,
    runtime_control: &PersistedRuntimeControl,
) -> Option<StrategyRuntimeHoldReason> {
    if route_state.stale.as_ref().map(|stale| stale.status)
        == Some(RouteReadinessStaleStatus::Stale)
    {
        return Some(StrategyRuntimeHoldReason::RouteReadinessStale);
    }
    if selection.approval.approved_revision != Some(selection.selection_revision) {
        return Some(StrategyRuntimeHoldReason::ApprovedSelectionRevisionStale);
    }
    if route_state.status != RouteReadinessStatus::Ready || route_state.current_step_key.is_some() {
        return Some(StrategyRuntimeHoldReason::RouteReadinessIncomplete);
    }
    match runtime_control.control_mode.as_str() {
        "paused" => Some(StrategyRuntimeHoldReason::RuntimeControlPaused),
        "stopped" => Some(StrategyRuntimeHoldReason::RuntimeControlStopped),
        _ => None,
    }
}

fn persisted_strategy_runtime_handoff_from_record(
    record: &StrategyRuntimeHandoffRecord,
) -> PersistedStrategyRuntimeHandoff {
    PersistedStrategyRuntimeHandoff {
        install_id: record.install_id.clone(),
        proposal_id: record.proposal_id.clone(),
        selection_id: record.selection_id.clone(),
        approved_selection_revision: record.approved_selection_revision,
        route_id: record.route_id.clone(),
        request_id: record.request_id.clone(),
        route_readiness_fingerprint: record.route_readiness_fingerprint.clone(),
        route_readiness_status: record.route_readiness_status.as_str().to_owned(),
        route_readiness_evaluated_at: record.route_readiness_evaluated_at.clone(),
        eligibility_status: record.eligibility_status.as_str().to_owned(),
        hold_reason: record
            .hold_reason
            .as_ref()
            .map(|reason| reason.as_str().to_owned()),
        runtime_control_mode: record.runtime_control_mode.clone(),
        strategy_id: record.strategy_id.clone(),
        runtime_identity_refreshed_at: record.runtime_identity_refreshed_at.clone(),
        runtime_identity_source: record.runtime_identity_source.clone(),
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
    }
}

fn invalidate_route_readiness_record(
    mut record: RouteReadinessRecord,
    evaluated_at: &str,
) -> RouteReadinessRecord {
    if !has_completed_owner_route_progress(&record) {
        return record;
    }

    let stale_reason =
        "route readiness facts changed and prior owner progress must be reviewed".to_owned();
    for step in &mut record.ordered_steps {
        if step.step_key == "review_stale_readiness" {
            step.status = RouteReadinessStepStatus::Pending;
            step.blocker_reason = Some(stale_reason.clone());
            step.recommended_action = Some(RouteReadinessActionRef {
                kind: RouteReadinessActionKind::ReviewStaleReadiness,
                step_key: Some("review_stale_readiness".to_owned()),
            });
            step.completed_at = None;
        } else if step.status == RouteReadinessStepStatus::Completed {
            step.status = RouteReadinessStepStatus::Stale;
            step.completed_at = None;
        }
    }
    record.stale = Some(RouteReadinessStaleState {
        status: RouteReadinessStaleStatus::Stale,
        reason: Some(stale_reason.clone()),
        detected_at: Some(evaluated_at.to_owned()),
    });
    record.ordered_steps = order_route_steps(record.ordered_steps);
    record.current_step_key = current_route_step_key(&record.ordered_steps);
    record.recommended_action = record
        .ordered_steps
        .iter()
        .find(|step| record.current_step_key.as_deref() == Some(step.step_key.as_str()))
        .and_then(|step| step.recommended_action.clone());
    record.evaluated_at = Some(evaluated_at.to_owned());
    record
}

fn build_route_readiness_record(
    install: &PersistedOnboardingInstall,
    envelope: &AgentRequestEnvelope<Intent>,
    route_decision: &a2ex_control::RouteDecision,
    proposal_id: &str,
    route_id: &str,
    request_id: &str,
    reserved_capital_usd: Option<u64>,
    evaluated_at: &str,
) -> Result<RouteReadinessRecord, RouteReadinessError> {
    let identity = RouteReadinessIdentity {
        install_id: install.install_id.clone(),
        proposal_id: proposal_id.to_owned(),
        route_id: route_id.to_owned(),
        request_id: request_id.to_owned(),
    };
    let compiled = a2ex_compiler::compile_intent(envelope).map_err(|error| {
        RouteReadinessError::Persistence {
            message: format!(
                "request {request_id} could not be recompiled for route readiness: {}",
                a2ex_compiler::format_compiler_failure(&error)
            ),
        }
    })?;
    let plan_preview = match route_decision.route {
        a2ex_control::RouteTarget::PlannedExecution => Some(
            a2ex_planner::plan_intent(&compiled, &a2ex_planner::CapabilityMatrix::m001_defaults())
                .map_err(|error| RouteReadinessError::Persistence {
                    message: error.to_string(),
                })?,
        ),
        _ => None,
    };
    let support_truth = project_route_support_truth(
        &a2ex_planner::CapabilityMatrix::m001_defaults(),
        &compiled,
        plan_preview.as_ref(),
        reserved_capital_usd,
    );
    let capital_evidence = vec![route_install_evidence(install)?];
    let capital = RouteCapitalReadiness {
        required_capital_usd: Some(support_truth.capital_support.required_capital_usd),
        available_capital_usd: support_truth.capital_support.available_capital_usd,
        reserved_capital_usd: support_truth.capital_support.reserved_capital_usd,
        completeness: support_truth.capital_support.completeness,
        summary: support_truth.capital_support.summary.clone(),
        reason: support_truth.capital_support.reason.clone(),
        evidence: capital_evidence.clone(),
    };
    let approvals = support_truth
        .approval_requirements
        .into_iter()
        .map(|requirement| RouteApprovalTuple {
            venue: requirement.venue.clone(),
            approval_type: requirement.approval_type.clone(),
            asset: requirement.asset.clone(),
            chain: requirement.chain.clone(),
            context: requirement.context.clone(),
            required: requirement.required,
            auth_summary: requirement.auth_summary.clone(),
            owner_action: requirement.required.then(|| {
                owner_action_for_approval(
                    &requirement.venue,
                    requirement.approval_type.as_str(),
                    requirement.asset.as_deref(),
                )
            }),
        })
        .collect::<Vec<_>>();

    let capital_incomplete = capital.completeness != ProposalQuantitativeCompleteness::Complete;
    let blockers = capital_incomplete
        .then(|| RouteReadinessBlocker {
            code: "capital_evidence_incomplete".to_owned(),
            summary: capital.reason.clone(),
            provenance: capital_evidence,
        })
        .into_iter()
        .collect::<Vec<_>>();
    let status = if blockers.is_empty() {
        RouteReadinessStatus::Ready
    } else {
        RouteReadinessStatus::Incomplete
    };
    let recommended_owner_action = if blockers.is_empty() {
        None
    } else {
        Some(RouteOwnerAction {
            kind: "fund_route_capital".to_owned(),
            summary: format!(
                "Confirm and fund {} route capital locally before retrying readiness.",
                envelope.payload.funding.preferred_asset
            ),
        })
    };

    let fingerprint = route_evaluation_fingerprint(
        request_id,
        &capital,
        &approvals,
        &blockers,
        recommended_owner_action.as_ref(),
    )?;

    Ok(RouteReadinessRecord {
        identity,
        status,
        capital,
        approvals,
        blockers,
        recommended_owner_action,
        ordered_steps: Vec::new(),
        current_step_key: None,
        recommended_action: None,
        stale: Some(RouteReadinessStaleState {
            status: RouteReadinessStaleStatus::Fresh,
            reason: None,
            detected_at: None,
        }),
        last_rejection: None,
        evaluation: Some(RouteReadinessEvaluationMetadata {
            request_id: request_id.to_owned(),
            evaluated_at: evaluated_at.to_owned(),
            fingerprint,
        }),
        evaluated_at: Some(evaluated_at.to_owned()),
    })
}

fn persisted_route_readiness_state_from_record(
    record: &RouteReadinessRecord,
    timestamp: &str,
) -> Result<(PersistedRouteReadiness, Vec<PersistedRouteReadinessStep>), RouteReadinessError> {
    let summary = PersistedRouteReadiness {
        install_id: record.identity.install_id.clone(),
        proposal_id: record.identity.proposal_id.clone(),
        route_id: record.identity.route_id.clone(),
        request_id: record.identity.request_id.clone(),
        status: record.status.as_str().to_owned(),
        capital: serde_json::to_value(&record.capital).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
        approvals: serde_json::to_value(&record.approvals).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
        blockers: serde_json::to_value(&record.blockers).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
        recommended_owner_action: record
            .recommended_owner_action
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?,
        ordered_steps: serde_json::to_value(&record.ordered_steps).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
        current_step_key: record.current_step_key.clone(),
        last_route_rejection_code: record
            .last_rejection
            .as_ref()
            .map(|value| value.code.clone()),
        last_route_rejection_message: record
            .last_rejection
            .as_ref()
            .map(|value| value.message.clone()),
        last_route_rejection_at: record
            .last_rejection
            .as_ref()
            .and_then(|value| value.observed_at.clone()),
        evaluation: record
            .evaluation
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?,
        evaluation_fingerprint: record
            .evaluation
            .as_ref()
            .map(|value| value.fingerprint.clone()),
        stale_status: record
            .stale
            .as_ref()
            .map(|value| value.status.as_str().to_owned())
            .unwrap_or_else(|| RouteReadinessStaleStatus::Fresh.as_str().to_owned()),
        stale_reason: record.stale.as_ref().and_then(|value| value.reason.clone()),
        stale_detected_at: record
            .stale
            .as_ref()
            .and_then(|value| value.detected_at.clone()),
        evaluated_at: record.evaluated_at.clone(),
        created_at: timestamp.to_owned(),
        updated_at: timestamp.to_owned(),
    };
    let steps = record
        .ordered_steps
        .iter()
        .map(|step| {
            Ok(PersistedRouteReadinessStep {
                install_id: record.identity.install_id.clone(),
                proposal_id: record.identity.proposal_id.clone(),
                route_id: record.identity.route_id.clone(),
                step_key: step.step_key.clone(),
                status: step.status.as_str().to_owned(),
                blocker_reason: step.blocker_reason.clone(),
                recommended_action: step
                    .recommended_action
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| RouteReadinessError::Persistence {
                        message: error.to_string(),
                    })?,
                completed_at: step.completed_at.clone(),
                created_at: timestamp.to_owned(),
                updated_at: timestamp.to_owned(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((summary, steps))
}

fn route_readiness_from_persisted(
    persisted: &PersistedRouteReadiness,
    persisted_steps: &[PersistedRouteReadinessStep],
) -> Result<RouteReadinessRecord, RouteReadinessError> {
    let mut ordered_steps = if persisted_steps.is_empty() {
        serde_json::from_value(persisted.ordered_steps.clone()).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?
    } else {
        persisted_steps
            .iter()
            .map(route_readiness_step_from_persisted)
            .collect::<Result<Vec<_>, _>>()?
    };
    ordered_steps = order_route_steps(ordered_steps);
    let current_step_key = current_route_step_key(&ordered_steps);
    let recommended_action = ordered_steps
        .iter()
        .find(|step| current_step_key.as_deref() == Some(step.step_key.as_str()))
        .and_then(|step| step.recommended_action.clone());

    Ok(RouteReadinessRecord {
        identity: RouteReadinessIdentity {
            install_id: persisted.install_id.clone(),
            proposal_id: persisted.proposal_id.clone(),
            route_id: persisted.route_id.clone(),
            request_id: persisted.request_id.clone(),
        },
        status: RouteReadinessStatus::from_str(&persisted.status).ok_or_else(|| {
            RouteReadinessError::Persistence {
                message: format!(
                    "unknown persisted route readiness status {}",
                    persisted.status
                ),
            }
        })?,
        capital: serde_json::from_value(persisted.capital.clone()).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
        approvals: serde_json::from_value(persisted.approvals.clone()).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
        blockers: serde_json::from_value(persisted.blockers.clone()).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
        recommended_owner_action: persisted
            .recommended_owner_action
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?,
        ordered_steps,
        current_step_key,
        recommended_action,
        stale: Some(RouteReadinessStaleState {
            status: RouteReadinessStaleStatus::from_str(&persisted.stale_status).ok_or_else(
                || RouteReadinessError::Persistence {
                    message: format!("unknown persisted stale status {}", persisted.stale_status),
                },
            )?,
            reason: persisted.stale_reason.clone(),
            detected_at: persisted.stale_detected_at.clone(),
        }),
        last_rejection: persisted.last_route_rejection_code.as_ref().map(|code| {
            RouteReadinessActionRejection {
                code: code.clone(),
                message: persisted
                    .last_route_rejection_message
                    .clone()
                    .unwrap_or_default(),
                observed_at: persisted.last_route_rejection_at.clone(),
            }
        }),
        evaluation: persisted
            .evaluation
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?,
        evaluated_at: persisted.evaluated_at.clone(),
    })
}

fn route_record_with_guided_state(
    mut base_record: RouteReadinessRecord,
    existing_state: Option<&(PersistedRouteReadiness, Vec<PersistedRouteReadinessStep>)>,
    evaluated_at: &str,
) -> Result<RouteReadinessRecord, RouteReadinessError> {
    let existing_steps = existing_state
        .map(|(_, steps)| steps.as_slice())
        .unwrap_or(&[]);
    let existing_record = existing_state
        .map(|(summary, steps)| route_readiness_from_persisted(summary, steps))
        .transpose()?;
    let fingerprint = base_record
        .evaluation
        .as_ref()
        .map(|value| value.fingerprint.clone())
        .unwrap_or_default();
    let previous_fingerprint = existing_record.as_ref().and_then(|value| {
        value
            .evaluation
            .as_ref()
            .map(|evaluation| evaluation.fingerprint.clone())
    });
    let prior_owner_progress_exists = existing_record
        .as_ref()
        .is_some_and(has_completed_owner_route_progress);
    let is_stale = prior_owner_progress_exists
        && previous_fingerprint
            .as_deref()
            .is_some_and(|previous| previous != fingerprint);
    let stale = if is_stale {
        RouteReadinessStaleState {
            status: RouteReadinessStaleStatus::Stale,
            reason: Some(
                "route readiness facts changed and prior owner progress must be reviewed"
                    .to_owned(),
            ),
            detected_at: Some(evaluated_at.to_owned()),
        }
    } else {
        RouteReadinessStaleState {
            status: RouteReadinessStaleStatus::Fresh,
            reason: None,
            detected_at: None,
        }
    };
    let mut ordered_steps = seed_route_steps(&base_record, existing_steps, is_stale, evaluated_at)?;
    ordered_steps = order_route_steps(ordered_steps);
    let current_step_key = current_route_step_key(&ordered_steps);
    let recommended_action = ordered_steps
        .iter()
        .find(|step| current_step_key.as_deref() == Some(step.step_key.as_str()))
        .and_then(|step| step.recommended_action.clone());
    base_record.ordered_steps = ordered_steps;
    base_record.current_step_key = current_step_key;
    base_record.recommended_action = recommended_action;
    base_record.stale = Some(stale);
    base_record.last_rejection = existing_record.and_then(|value| value.last_rejection);
    Ok(base_record)
}

fn has_completed_owner_route_progress(record: &RouteReadinessRecord) -> bool {
    record.ordered_steps.iter().any(|step| {
        step.step_key != "review_stale_readiness"
            && step.status == RouteReadinessStepStatus::Completed
    })
}

fn seed_route_steps(
    record: &RouteReadinessRecord,
    existing_steps: &[PersistedRouteReadinessStep],
    is_stale: bool,
    evaluated_at: &str,
) -> Result<Vec<RouteReadinessStep>, RouteReadinessError> {
    let existing = existing_steps
        .iter()
        .map(route_readiness_step_from_persisted)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|step| (step.step_key.clone(), step))
        .collect::<BTreeMap<_, _>>();

    let capital_blocker = record
        .blockers
        .iter()
        .find(|blocker| blocker.code == "capital_evidence_incomplete")
        .map(|blocker| blocker.summary.clone());
    let approvals_required = record.approvals.iter().any(|approval| approval.required);
    let approval_blocker = approvals_required.then(|| {
        format!(
            "{} route approval tuple(s) still require explicit local owner confirmation",
            record
                .approvals
                .iter()
                .filter(|approval| approval.required)
                .count()
        )
    });

    let mut steps = vec![
        merge_seeded_route_step(
            RouteReadinessStep {
                step_key: "fund_route_capital".to_owned(),
                status: if capital_blocker.is_some() {
                    RouteReadinessStepStatus::Pending
                } else {
                    RouteReadinessStepStatus::Completed
                },
                blocker_reason: capital_blocker,
                recommended_action: Some(RouteReadinessActionRef {
                    kind: RouteReadinessActionKind::CompleteStep,
                    step_key: Some("fund_route_capital".to_owned()),
                }),
                completed_at: None,
            },
            existing.get("fund_route_capital"),
            is_stale,
            evaluated_at,
        ),
        merge_seeded_route_step(
            RouteReadinessStep {
                step_key: "satisfy_route_approvals".to_owned(),
                status: if approvals_required {
                    RouteReadinessStepStatus::Pending
                } else {
                    RouteReadinessStepStatus::Completed
                },
                blocker_reason: approval_blocker,
                recommended_action: Some(RouteReadinessActionRef {
                    kind: RouteReadinessActionKind::CompleteStep,
                    step_key: Some("satisfy_route_approvals".to_owned()),
                }),
                completed_at: None,
            },
            existing.get("satisfy_route_approvals"),
            is_stale,
            evaluated_at,
        ),
    ];
    let stale_step = if is_stale {
        RouteReadinessStep {
            step_key: "review_stale_readiness".to_owned(),
            status: RouteReadinessStepStatus::Pending,
            blocker_reason: Some(
                "route readiness facts changed and prior owner progress must be reviewed"
                    .to_owned(),
            ),
            recommended_action: Some(RouteReadinessActionRef {
                kind: RouteReadinessActionKind::ReviewStaleReadiness,
                step_key: Some("review_stale_readiness".to_owned()),
            }),
            completed_at: None,
        }
    } else {
        RouteReadinessStep {
            step_key: "review_stale_readiness".to_owned(),
            status: RouteReadinessStepStatus::Completed,
            blocker_reason: None,
            recommended_action: None,
            completed_at: Some(evaluated_at.to_owned()),
        }
    };
    steps.push(merge_seeded_route_step(
        stale_step,
        existing.get("review_stale_readiness"),
        false,
        evaluated_at,
    ));
    Ok(steps)
}

fn merge_seeded_route_step(
    mut seeded: RouteReadinessStep,
    existing: Option<&RouteReadinessStep>,
    is_stale: bool,
    evaluated_at: &str,
) -> RouteReadinessStep {
    if seeded.step_key == "review_stale_readiness" {
        return seeded;
    }
    if is_stale {
        seeded.status = RouteReadinessStepStatus::Stale;
        seeded.completed_at = None;
        return seeded;
    }
    if let Some(existing) = existing
        && existing.status == RouteReadinessStepStatus::Completed
    {
        seeded.status = RouteReadinessStepStatus::Completed;
        seeded.completed_at = existing
            .completed_at
            .clone()
            .or_else(|| Some(evaluated_at.to_owned()));
    }
    seeded
}

fn order_route_steps(mut steps: Vec<RouteReadinessStep>) -> Vec<RouteReadinessStep> {
    steps.sort_by(|left, right| {
        route_step_rank(left)
            .cmp(&route_step_rank(right))
            .then_with(|| left.step_key.cmp(&right.step_key))
    });
    steps
}

fn route_step_rank(step: &RouteReadinessStep) -> (u8, &str) {
    let priority = match step.status {
        RouteReadinessStepStatus::Stale => {
            if step.step_key == "review_stale_readiness" {
                0
            } else {
                1
            }
        }
        RouteReadinessStepStatus::Pending => match step.step_key.as_str() {
            "fund_route_capital" => 2,
            "satisfy_route_approvals" => 3,
            _ => 4,
        },
        RouteReadinessStepStatus::Completed => 5,
    };
    (priority, step.step_key.as_str())
}

fn current_route_step_key(steps: &[RouteReadinessStep]) -> Option<String> {
    steps
        .iter()
        .find(|step| {
            matches!(
                step.status,
                RouteReadinessStepStatus::Pending | RouteReadinessStepStatus::Stale
            )
        })
        .map(|step| step.step_key.clone())
}

fn route_readiness_step_from_persisted(
    step: &PersistedRouteReadinessStep,
) -> Result<RouteReadinessStep, RouteReadinessError> {
    Ok(RouteReadinessStep {
        step_key: step.step_key.clone(),
        status: RouteReadinessStepStatus::from_str(&step.status).ok_or_else(|| {
            RouteReadinessError::Persistence {
                message: format!("unknown persisted route step status {}", step.status),
            }
        })?,
        blocker_reason: step.blocker_reason.clone(),
        recommended_action: step
            .recommended_action
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| RouteReadinessError::Persistence {
                message: error.to_string(),
            })?,
        completed_at: step.completed_at.clone(),
    })
}

fn route_evaluation_fingerprint(
    request_id: &str,
    capital: &RouteCapitalReadiness,
    approvals: &[RouteApprovalTuple],
    blockers: &[RouteReadinessBlocker],
    recommended_owner_action: Option<&RouteOwnerAction>,
) -> Result<String, RouteReadinessError> {
    let payload = serde_json::json!({
        "request_id": request_id,
        "capital": capital,
        "approvals": approvals,
        "blockers": blockers,
        "recommended_owner_action": recommended_owner_action,
    });
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|error| RouteReadinessError::Persistence {
            message: error.to_string(),
        })?;
    Ok(format!("{:x}", Sha256::digest(payload_bytes)))
}

fn validate_route_current_step(
    current_step_key: Option<&str>,
    expected_step_key: Option<&str>,
    mismatch_code: &str,
    mismatch_message: &str,
) -> Result<(), RouteReadinessError> {
    if current_step_key == expected_step_key {
        return Ok(());
    }

    Err(RouteReadinessError::ActionRejected {
        code: mismatch_code.to_owned(),
        message: mismatch_message.to_owned(),
    })
}

fn route_mismatch_action_code(current_step_key: Option<&str>) -> &'static str {
    match current_step_key {
        Some("review_stale_readiness") => "stale_review_required",
        Some(_) => "out_of_order_action",
        None => "route_already_ready",
    }
}

fn route_mismatch_action_message(current_step_key: Option<&str>) -> &'static str {
    match current_step_key {
        Some("review_stale_readiness") => {
            "stale route readiness must be reviewed before later steps can proceed"
        }
        Some(_) => "the requested route readiness action does not match the current guided step",
        None => "route readiness has no current step to mutate",
    }
}

impl RouteReadinessRecord {
    fn into_action_result(self) -> RouteReadinessActionResult {
        RouteReadinessActionResult {
            install_id: self.identity.install_id,
            proposal_id: self.identity.proposal_id,
            route_id: self.identity.route_id,
            status: self.status,
            current_step_key: self.current_step_key,
            recommended_action: self.recommended_action,
            stale: self.stale.unwrap_or(RouteReadinessStaleState {
                status: RouteReadinessStaleStatus::Fresh,
                reason: None,
                detected_at: None,
            }),
        }
    }
}

fn route_install_evidence(
    install: &PersistedOnboardingInstall,
) -> Result<InterpretationEvidence, RouteReadinessError> {
    Ok(InterpretationEvidence {
        document_id: "install_state".to_owned(),
        section_id: Some(install.install_id.clone()),
        section_slug: Some("canonical_route_readiness".to_owned()),
        source_url: Url::parse(&install.attached_bundle_url).map_err(|error| {
            RouteReadinessError::Persistence {
                message: error.to_string(),
            }
        })?,
    })
}

fn owner_action_for_approval(venue: &str, approval_type: &str, asset: Option<&str>) -> String {
    match (venue, approval_type, asset) {
        ("across", "erc20_allowance", Some(asset)) => {
            format!("approve_local_allowance:{asset}:before_bridge")
        }
        ("kalshi", "api_request_signature", _) => "confirm_local_kalshi_auth_ready".to_owned(),
        ("hyperliquid", "exchange_order_signature", _) => {
            "confirm_local_hedge_signing_ready".to_owned()
        }
        _ => format!("satisfy_{venue}_{approval_type}"),
    }
}

fn validate_current_step(
    current_step_key: Option<&str>,
    expected_step_key: Option<&str>,
    mismatch_code: &str,
    mismatch_message: &str,
) -> Result<(), GuidedOnboardingError> {
    if current_step_key == expected_step_key {
        return Ok(());
    }

    Err(GuidedOnboardingError::ActionRejected {
        code: mismatch_code.to_owned(),
        message: mismatch_message.to_owned(),
    })
}

fn mismatch_action_code(current_step_key: Option<&str>) -> &'static str {
    match current_step_key {
        Some("bundle_drift") => "bundle_drift_review_required",
        Some(_) => "out_of_order_action",
        None => "onboarding_already_ready",
    }
}

fn mismatch_action_message(current_step_key: Option<&str>) -> &'static str {
    match current_step_key {
        Some("bundle_drift") => {
            "bundle drift review must be acknowledged before later onboarding steps can proceed"
        }
        Some(_) => "the requested onboarding action does not match the current guided step",
        None => "guided onboarding has no current step to mutate",
    }
}

fn guided_state_from_parts(
    install: &a2ex_state::PersistedOnboardingInstall,
    persisted_items: &[PersistedOnboardingChecklistItem],
) -> Result<GuidedOnboardingState, GuidedOnboardingError> {
    Ok(guided_inspection_from_parts(install, persisted_items)?.into())
}

fn guided_inspection_from_parts(
    install: &a2ex_state::PersistedOnboardingInstall,
    persisted_items: &[PersistedOnboardingChecklistItem],
) -> Result<GuidedOnboardingInspection, GuidedOnboardingError> {
    let aggregate_status = OnboardingAggregateStatus::from_str(&install.onboarding_status)
        .ok_or_else(|| GuidedOnboardingError::Persistence {
            message: format!(
                "unknown persisted onboarding status {}",
                install.onboarding_status
            ),
        })?;
    let attached_bundle_url = parse_guided_url(&install.attached_bundle_url)?;
    let drift = install
        .bundle_drift
        .as_ref()
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| GuidedOnboardingError::Persistence {
            message: error.to_string(),
        })?;
    let checklist_items = persisted_items
        .iter()
        .filter(|item| !is_bundle_state_item(item))
        .map(onboarding_checklist_item_from_persisted)
        .collect::<Result<Vec<_>, _>>()?;
    let ordered_steps = ordered_guided_steps(&checklist_items);
    let current_step = ordered_steps.iter().find(|step| {
        step.status == OnboardingChecklistItemStatus::Pending
            || step.status == OnboardingChecklistItemStatus::Blocked
            || step.status == OnboardingChecklistItemStatus::Drifted
    });
    let current_step_key = current_step.map(|step| step.step_key.clone());
    let recommended_action = current_step.and_then(|step| step.recommended_action.clone());
    let proposal_handoff = (aggregate_status == OnboardingAggregateStatus::Ready)
        .then(|| ProposalHandoff::from_attached_bundle_url(attached_bundle_url.clone()));
    let last_rejection = install.last_onboarding_rejection_code.as_ref().map(|code| {
        GuidedOnboardingActionRejection {
            code: code.clone(),
            message: install
                .last_onboarding_rejection_message
                .clone()
                .unwrap_or_default(),
            observed_at: install.last_onboarding_rejection_at.clone(),
        }
    });

    Ok(GuidedOnboardingInspection {
        install_id: install.install_id.clone(),
        workspace_id: install.workspace_id.clone(),
        attached_bundle_url,
        bootstrap: bootstrap_report_from_install(install)?,
        aggregate_status,
        ordered_steps,
        current_step_key,
        recommended_action,
        proposal_handoff,
        checklist_items,
        drift,
        last_rejection,
    })
}

fn ordered_guided_steps(checklist_items: &[OnboardingChecklistItem]) -> Vec<GuidedOnboardingStep> {
    let mut steps = checklist_items
        .iter()
        .map(|item| GuidedOnboardingStep {
            step_key: item.checklist_key.clone(),
            status: item.status,
            blocker_reason: item.blocker_reason.clone(),
            recommended_action: guided_action_for_item(item),
        })
        .collect::<Vec<_>>();
    steps.sort_by(|left, right| {
        guided_step_rank(left)
            .cmp(&guided_step_rank(right))
            .then_with(|| left.step_key.cmp(&right.step_key))
    });
    steps
}

fn guided_step_rank(step: &GuidedOnboardingStep) -> (u8, &str) {
    let priority = match step.status {
        OnboardingChecklistItemStatus::Drifted => 0,
        OnboardingChecklistItemStatus::Blocked => 1,
        OnboardingChecklistItemStatus::Pending => {
            match step.recommended_action.as_ref().map(|action| action.kind) {
                Some(GuidedOnboardingActionKind::CompleteStep) => 2,
                Some(GuidedOnboardingActionKind::ResolveOwnerDecision) => 3,
                Some(GuidedOnboardingActionKind::Refresh)
                | Some(GuidedOnboardingActionKind::AcknowledgeBundleDrift)
                | None => 4,
            }
        }
        OnboardingChecklistItemStatus::Completed => 5,
    };
    (priority, step.step_key.as_str())
}

fn guided_action_for_item(item: &OnboardingChecklistItem) -> Option<GuidedOnboardingActionRef> {
    match item.status {
        OnboardingChecklistItemStatus::Pending => match item.source_kind {
            OnboardingChecklistSourceKind::SetupRequirement => Some(GuidedOnboardingActionRef {
                kind: GuidedOnboardingActionKind::CompleteStep,
                step_key: Some(item.checklist_key.clone()),
            }),
            OnboardingChecklistSourceKind::OwnerDecision => Some(GuidedOnboardingActionRef {
                kind: GuidedOnboardingActionKind::ResolveOwnerDecision,
                step_key: Some(item.checklist_key.clone()),
            }),
            _ => Some(GuidedOnboardingActionRef {
                kind: GuidedOnboardingActionKind::Refresh,
                step_key: Some(item.checklist_key.clone()),
            }),
        },
        OnboardingChecklistItemStatus::Drifted => Some(GuidedOnboardingActionRef {
            kind: GuidedOnboardingActionKind::AcknowledgeBundleDrift,
            step_key: Some(item.checklist_key.clone()),
        }),
        OnboardingChecklistItemStatus::Blocked => Some(GuidedOnboardingActionRef {
            kind: GuidedOnboardingActionKind::Refresh,
            step_key: Some(item.checklist_key.clone()),
        }),
        OnboardingChecklistItemStatus::Completed => None,
    }
}

fn onboarding_checklist_item_from_persisted(
    item: &PersistedOnboardingChecklistItem,
) -> Result<OnboardingChecklistItem, GuidedOnboardingError> {
    Ok(OnboardingChecklistItem {
        checklist_key: item.checklist_key.clone(),
        source_kind: OnboardingChecklistSourceKind::from_str(&item.source_kind).ok_or_else(
            || GuidedOnboardingError::Persistence {
                message: format!(
                    "unknown persisted onboarding source kind {}",
                    item.source_kind
                ),
            },
        )?,
        status: OnboardingChecklistItemStatus::from_str(&item.status).ok_or_else(|| {
            GuidedOnboardingError::Persistence {
                message: format!("unknown persisted onboarding status {}", item.status),
            }
        })?,
        blocker_reason: item.blocker_reason.clone(),
        next_action: item.next_action.clone(),
        evidence: serde_json::from_value(item.evidence.clone()).map_err(|error| {
            GuidedOnboardingError::Persistence {
                message: error.to_string(),
            }
        })?,
        completed_at: item.completed_at.clone(),
        updated_at: Some(item.updated_at.clone()),
    })
}

fn derive_guided_aggregate_status(
    install: &a2ex_state::PersistedOnboardingInstall,
    persisted_items: &[PersistedOnboardingChecklistItem],
    drift: Option<&OnboardingBundleDrift>,
) -> OnboardingAggregateStatus {
    let items = persisted_items
        .iter()
        .filter(|item| !is_bundle_state_item(item))
        .map(|item| (item.status.as_str(), item.checklist_key.as_str()));

    if drift.is_some() {
        return OnboardingAggregateStatus::Drifted;
    }
    if items
        .clone()
        .any(|(status, _)| status == OnboardingChecklistItemStatus::Blocked.as_str())
    {
        return OnboardingAggregateStatus::Blocked;
    }
    if items.clone().any(|(status, key)| {
        status == OnboardingChecklistItemStatus::Pending.as_str()
            && key != BUNDLE_STATE_CHECKLIST_KEY
    }) {
        return OnboardingAggregateStatus::NeedsAction;
    }
    if InstallReadiness::status_from_str(&install.readiness_status)
        == Some(a2ex_skill_bundle::SkillBundleInterpretationStatus::Blocked)
    {
        return OnboardingAggregateStatus::Blocked;
    }
    OnboardingAggregateStatus::Ready
}

fn persisted_checklist_item(
    install_id: &str,
    item: &OnboardingChecklistItem,
    load_outcome: &BundleLoadOutcome,
    now: &str,
) -> Result<PersistedOnboardingChecklistItem, InstallBootstrapError> {
    Ok(PersistedOnboardingChecklistItem {
        install_id: install_id.to_owned(),
        checklist_key: item.checklist_key.clone(),
        source_kind: item.source_kind.as_str().to_owned(),
        status: item.status.as_str().to_owned(),
        blocker_reason: item.blocker_reason.clone(),
        next_action: item.next_action.clone(),
        evidence: serde_json::to_value(&item.evidence).map_err(|error| {
            InstallBootstrapError::Persistence {
                message: error.to_string(),
            }
        })?,
        lifecycle: Some(persisted_lifecycle_value(
            item_source_document_ids(item),
            load_outcome,
        )?),
        completed_at: item.completed_at.clone(),
        created_at: now.to_owned(),
        updated_at: item.updated_at.clone().unwrap_or_else(|| now.to_owned()),
    })
}

fn bundle_state_persisted_item(
    install_id: &str,
    load_outcome: &BundleLoadOutcome,
    now: &str,
) -> Result<PersistedOnboardingChecklistItem, InstallBootstrapError> {
    Ok(PersistedOnboardingChecklistItem {
        install_id: install_id.to_owned(),
        checklist_key: BUNDLE_STATE_CHECKLIST_KEY.to_owned(),
        source_kind: OnboardingChecklistSourceKind::BundleDrift
            .as_str()
            .to_owned(),
        status: OnboardingChecklistItemStatus::Completed.as_str().to_owned(),
        blocker_reason: None,
        next_action: None,
        evidence: serde_json::Value::Array(Vec::new()),
        lifecycle: Some(persisted_lifecycle_value(Vec::new(), load_outcome)?),
        completed_at: None,
        created_at: now.to_owned(),
        updated_at: now.to_owned(),
    })
}

fn persisted_lifecycle_value(
    source_document_ids: Vec<String>,
    load_outcome: &BundleLoadOutcome,
) -> Result<serde_json::Value, InstallBootstrapError> {
    serde_json::to_value(PersistedChecklistLifecycle {
        source_document_ids,
        load_outcome: load_outcome.clone(),
    })
    .map_err(|error| InstallBootstrapError::Persistence {
        message: error.to_string(),
    })
}

fn project_checklist_items(
    interpretation: &SkillBundleInterpretation,
    drift: Option<&OnboardingBundleDrift>,
) -> Vec<OnboardingChecklistItem> {
    let mut items = Vec::new();

    for requirement in &interpretation.setup_requirements {
        items.push(OnboardingChecklistItem {
            checklist_key: requirement.requirement_key.clone(),
            source_kind: OnboardingChecklistSourceKind::SetupRequirement,
            status: OnboardingChecklistItemStatus::Pending,
            blocker_reason: None,
            next_action: Some(next_action_for_requirement(requirement).to_owned()),
            evidence: requirement_evidence(requirement),
            completed_at: None,
            updated_at: Some(current_timestamp()),
        });
    }

    for decision in &interpretation.owner_decisions {
        items.push(OnboardingChecklistItem {
            checklist_key: decision.decision_key.clone(),
            source_kind: OnboardingChecklistSourceKind::OwnerDecision,
            status: OnboardingChecklistItemStatus::Pending,
            blocker_reason: None,
            next_action: Some("resolve_owner_decision".to_owned()),
            evidence: decision_evidence(decision),
            completed_at: None,
            updated_at: Some(current_timestamp()),
        });
    }

    for blocker in &interpretation.blockers {
        items.push(OnboardingChecklistItem {
            checklist_key: blocker.blocker_key.clone(),
            source_kind: OnboardingChecklistSourceKind::ReadinessBlocker,
            status: OnboardingChecklistItemStatus::Blocked,
            blocker_reason: Some(blocker.summary.clone()),
            next_action: Some("inspect_bundle_blocker".to_owned()),
            evidence: blocker_evidence(blocker),
            completed_at: None,
            updated_at: Some(current_timestamp()),
        });
    }

    if let Some(drift) = drift {
        items.push(bundle_drift_item(drift));
    }

    items.sort_by(|left, right| left.checklist_key.cmp(&right.checklist_key));
    items
}

fn merge_with_existing_items(
    projected_items: Vec<OnboardingChecklistItem>,
    existing_items: Vec<PersistedOnboardingChecklistItem>,
) -> Result<Vec<OnboardingChecklistItem>, InstallBootstrapError> {
    let mut existing_by_key = BTreeMap::new();
    for item in existing_items {
        existing_by_key.insert(item.checklist_key.clone(), item);
    }

    projected_items
        .into_iter()
        .map(|mut projected| {
            if let Some(existing) = existing_by_key.get(&projected.checklist_key)
                && existing.source_kind == projected.source_kind.as_str()
                && existing.status == OnboardingChecklistItemStatus::Completed.as_str()
                && projected.status == OnboardingChecklistItemStatus::Pending
            {
                projected.status = OnboardingChecklistItemStatus::Completed;
                projected.completed_at = existing.completed_at.clone();
                projected.updated_at = existing
                    .completed_at
                    .clone()
                    .or_else(|| Some(current_timestamp()));
            }
            Ok(projected)
        })
        .collect()
}

fn is_bundle_state_item(item: &PersistedOnboardingChecklistItem) -> bool {
    item.checklist_key == BUNDLE_STATE_CHECKLIST_KEY
}

fn previous_load_outcome(
    existing_items: &[PersistedOnboardingChecklistItem],
) -> Result<Option<BundleLoadOutcome>, InstallBootstrapError> {
    existing_items
        .iter()
        .find(|item| is_bundle_state_item(item) && item.lifecycle.is_some())
        .or_else(|| existing_items.iter().find(|item| item.lifecycle.is_some()))
        .and_then(|item| item.lifecycle.as_ref())
        .map(|value| {
            serde_json::from_value::<PersistedChecklistLifecycle>(value.clone())
                .map(|persisted| persisted.load_outcome)
                .map_err(|error| InstallBootstrapError::Persistence {
                    message: error.to_string(),
                })
        })
        .transpose()
}

fn bundle_drift(
    previous: Option<&BundleLoadOutcome>,
    current: &BundleLoadOutcome,
) -> Option<OnboardingBundleDrift> {
    let previous = previous?;
    let lifecycle = current.lifecycle_change_from(Some(previous));
    if lifecycle.classification == BundleLifecycleClassification::NoChange {
        return None;
    }

    Some(OnboardingBundleDrift {
        classification: lifecycle.classification,
        changed_documents: lifecycle.changed_documents,
        diagnostics: lifecycle.diagnostics,
    })
}

fn item_source_document_ids(item: &OnboardingChecklistItem) -> Vec<String> {
    let mut document_ids = BTreeSet::new();
    for evidence in &item.evidence {
        document_ids.insert(evidence.document_id.clone());
    }
    document_ids.into_iter().collect()
}

fn bundle_drift_item(drift: &OnboardingBundleDrift) -> OnboardingChecklistItem {
    OnboardingChecklistItem {
        checklist_key: "bundle_drift".to_owned(),
        source_kind: OnboardingChecklistSourceKind::BundleDrift,
        status: OnboardingChecklistItemStatus::Drifted,
        blocker_reason: Some(bundle_drift_summary(drift)),
        next_action: Some("review_bundle_drift".to_owned()),
        evidence: bundle_drift_evidence(drift),
        completed_at: None,
        updated_at: Some(current_timestamp()),
    }
}

fn bundle_drift_summary(drift: &OnboardingBundleDrift) -> String {
    if let Some(diagnostic) = drift.diagnostics.first() {
        return diagnostic.message.clone();
    }
    if let Some(change) = drift.changed_documents.first() {
        return format!(
            "bundle document '{}' changed since the last onboarding refresh",
            change.document_id
        );
    }
    "bundle changed since the last onboarding refresh".to_owned()
}

fn bundle_drift_evidence(drift: &OnboardingBundleDrift) -> Vec<OnboardingEvidenceRef> {
    let mut evidence = Vec::new();

    for change in &drift.changed_documents {
        if let Some(source_url) = change.source_url.clone() {
            evidence.push(OnboardingEvidenceRef {
                kind: OnboardingEvidenceKind::DocumentReference,
                document_id: change.document_id.clone(),
                section_id: None,
                section_slug: None,
                source_url,
                redacted_summary: Some(format!(
                    "bundle_drift:{}",
                    lifecycle_change_kind_label(change.kind)
                )),
            });
        }
    }

    for diagnostic in &drift.diagnostics {
        if let (Some(document_id), Some(source_url)) = (
            diagnostic.document_id.clone(),
            diagnostic.source_url.clone(),
        ) {
            evidence.push(OnboardingEvidenceRef {
                kind: OnboardingEvidenceKind::DiagnosticReference,
                document_id,
                section_id: None,
                section_slug: None,
                source_url,
                redacted_summary: Some("bundle_drift:blocking_diagnostic".to_owned()),
            });
        }
    }

    evidence
}

fn lifecycle_change_kind_label(kind: BundleDocumentLifecycleChangeKind) -> &'static str {
    match kind {
        BundleDocumentLifecycleChangeKind::Added => "added",
        BundleDocumentLifecycleChangeKind::Removed => "removed",
        BundleDocumentLifecycleChangeKind::RevisionChanged => "revision_changed",
        BundleDocumentLifecycleChangeKind::ContentChanged => "content_changed",
    }
}

fn readiness_from_install(
    install: &PersistedOnboardingInstall,
) -> Result<InstallReadiness, InstallBootstrapError> {
    Ok(InstallReadiness {
        status: crate::model::InstallReadiness::status_from_str(&install.readiness_status)
            .ok_or_else(|| InstallBootstrapError::Persistence {
                message: format!(
                    "unknown persisted readiness status {}",
                    install.readiness_status
                ),
            })?,
        blockers: install.readiness_blockers.clone(),
        diagnostics: install.readiness_diagnostics.clone(),
    })
}

fn derive_aggregate_status(
    readiness: &InstallReadiness,
    items: &[OnboardingChecklistItem],
    drift: Option<&OnboardingBundleDrift>,
) -> OnboardingAggregateStatus {
    if drift.is_some() {
        return OnboardingAggregateStatus::Drifted;
    }
    if items
        .iter()
        .any(|item| item.status == OnboardingChecklistItemStatus::Blocked)
    {
        return OnboardingAggregateStatus::Blocked;
    }
    if items
        .iter()
        .any(|item| item.status == OnboardingChecklistItemStatus::Pending)
    {
        return OnboardingAggregateStatus::NeedsAction;
    }
    if readiness.status == a2ex_skill_bundle::SkillBundleInterpretationStatus::Blocked {
        return OnboardingAggregateStatus::Blocked;
    }
    OnboardingAggregateStatus::Ready
}

fn next_action_for_requirement(requirement: &InterpretationSetupRequirement) -> &'static str {
    match requirement.requirement_kind {
        InterpretationSetupRequirementKind::Secret => "provide_local_secret",
        InterpretationSetupRequirementKind::Environment => "configure_local_environment",
        InterpretationSetupRequirementKind::Approval => "request_owner_approval",
        InterpretationSetupRequirementKind::Unknown => "investigate_requirement",
    }
}

fn requirement_evidence(
    requirement: &InterpretationSetupRequirement,
) -> Vec<OnboardingEvidenceRef> {
    evidence_refs(
        &requirement.evidence,
        OnboardingEvidenceKind::RequirementIdentity,
        Some(format!(
            "requirement_identity:{}",
            requirement.requirement_key
        )),
    )
}

fn decision_evidence(decision: &InterpretationOwnerDecision) -> Vec<OnboardingEvidenceRef> {
    evidence_refs(
        &decision.evidence,
        OnboardingEvidenceKind::DecisionIdentity,
        Some(format!("decision_identity:{}", decision.decision_key)),
    )
}

fn blocker_evidence(blocker: &InterpretationBlocker) -> Vec<OnboardingEvidenceRef> {
    evidence_refs(
        &blocker.evidence,
        OnboardingEvidenceKind::DiagnosticReference,
        Some(format!("diagnostic_blocker:{}", blocker.blocker_key)),
    )
}

fn evidence_refs(
    evidence: &[InterpretationEvidence],
    kind: OnboardingEvidenceKind,
    redacted_summary: Option<String>,
) -> Vec<OnboardingEvidenceRef> {
    evidence
        .iter()
        .map(|item| OnboardingEvidenceRef {
            kind,
            document_id: item.document_id.clone(),
            section_id: item.section_id.clone(),
            section_slug: item.section_slug.clone(),
            source_url: item.source_url.clone(),
            redacted_summary: redacted_summary.clone(),
        })
        .collect()
}

fn bootstrap_report_from_install(
    install: &PersistedOnboardingInstall,
) -> Result<BootstrapReport, GuidedOnboardingError> {
    Ok(BootstrapReport {
        source: BootstrapSource::from_str(&install.bootstrap_source).ok_or_else(|| {
            GuidedOnboardingError::Persistence {
                message: format!(
                    "unknown persisted bootstrap source {}",
                    install.bootstrap_source
                ),
            }
        })?,
        bootstrap_path: install.bootstrap_path.clone(),
        state_db_path: PathBuf::from(&install.state_db_path),
        analytics_db_path: PathBuf::from(&install.analytics_db_path),
        used_remote_control_plane: install.used_remote_control_plane,
        recovered_existing_state: install.recovered_existing_state,
    })
}

fn parse_guided_url(value: &str) -> Result<Url, GuidedOnboardingError> {
    Url::parse(value).map_err(|error| GuidedOnboardingError::Persistence {
        message: error.to_string(),
    })
}

fn parse_url(value: &str) -> Result<Url, InstallBootstrapError> {
    Url::parse(value).map_err(|error| InstallBootstrapError::Persistence {
        message: error.to_string(),
    })
}

fn canonical_workspace_root(path: &Path) -> Result<PathBuf, InstallBootstrapError> {
    std::fs::create_dir_all(path).map_err(|error| InstallBootstrapError::Persistence {
        message: error.to_string(),
    })?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| InstallBootstrapError::Persistence {
                message: error.to_string(),
            })?
            .join(path)
    };
    Ok(absolute)
}

fn deterministic_id(prefix: &str, components: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update([0]);
    for component in components {
        hasher.update(component.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    format!("{prefix}-{}", hex::encode(&digest[..16]))
}

fn current_timestamp() -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    format_rfc3339_utc(epoch)
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
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}
