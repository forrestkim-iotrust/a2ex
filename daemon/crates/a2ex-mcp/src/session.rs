use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use a2ex_skill_bundle::{
    BundleDiagnosticCode, BundleError, BundleLifecycleChange, BundleLoadOutcome,
    InterpretationAmbiguity, InterpretationBlocker, InterpretationEvidence,
    InterpretationOwnerDecision, InterpretationSetupRequirement, ProposalQuantitativeCompleteness,
    SkillBundleInterpretation, SkillBundleInterpretationStatus, SkillProposalPacket,
    generate_proposal_packet,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::server::SessionHandoff;
use crate::{
    McpContractError, SessionCommandDisposition, SessionCommandOutcome, SessionCommandRejection,
    SessionFailureEvidence, SessionFailureKind, SessionNextOperatorStep,
    SessionNextOperatorStepKind, SessionProposalCompleteness, SessionProposalReadiness,
    SessionStopState, SkillSessionBundleResource, SkillSessionFailuresResource,
    SkillSessionLifecycleResource, SkillSessionLifecycleSummary, SkillSessionOperatorStateResource,
    SkillSessionResourceKind, SkillSessionStatusResource, session_uri_root, stable_session_id,
};

#[derive(Debug, Clone, Default)]
pub struct SkillSessionRegistry {
    sessions: Arc<RwLock<BTreeMap<String, SkillSessionSnapshot>>>,
}

impl SkillSessionRegistry {
    pub fn upsert(
        &self,
        action: SessionAction,
        entry_url: String,
        outcome: BundleLoadOutcome,
        interpretation: SkillBundleInterpretation,
        handoff: Option<SessionHandoff>,
    ) -> SkillSessionSnapshot {
        let session_id = stable_session_id(&entry_url);
        let mut sessions = self.sessions.write().expect("session registry write lock");
        let previous_snapshot = sessions.get(&session_id).cloned();
        let previous_outcome = previous_snapshot
            .as_ref()
            .map(|snapshot| snapshot.outcome.clone());
        let revision = previous_snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision + 1)
            .unwrap_or(1);
        let now_ms = unix_timestamp_ms();
        let lifecycle = outcome.lifecycle_change_from(previous_outcome.as_ref());

        let snapshot = SkillSessionSnapshot {
            session_id: session_id.clone(),
            entry_url,
            session_uri_root: session_uri_root(&session_id),
            revision,
            updated_at_ms: now_ms,
            last_operation: SessionOperationRecord {
                action: action.clone(),
                observed_at_ms: now_ms,
            },
            control: previous_snapshot
                .as_ref()
                .map(|snapshot| snapshot.control.clone())
                .unwrap_or_default(),
            last_command_outcome: SessionCommandOutcome {
                command: action.command_name().to_owned(),
                observed_at_ms: now_ms,
                disposition: SessionCommandDisposition::Succeeded,
                rejection_code: None,
                message: None,
            },
            last_rejected_command: previous_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.last_rejected_command.clone()),
            handoff: handoff.or_else(|| {
                previous_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.handoff.clone())
            }),
            lifecycle,
            outcome,
            interpretation,
        };

        sessions.insert(session_id, snapshot.clone());
        snapshot
    }

    pub fn get(&self, session_id: &str) -> Result<SkillSessionSnapshot, McpContractError> {
        self.sessions
            .read()
            .expect("session registry read lock")
            .get(session_id)
            .cloned()
            .ok_or_else(|| McpContractError::UnknownSession {
                session_id: session_id.to_owned(),
            })
    }

    pub fn stop_session(&self, session_id: &str) -> Result<SkillSessionSnapshot, McpContractError> {
        self.mutate(session_id, |snapshot, now_ms| {
            snapshot.revision += 1;
            snapshot.updated_at_ms = now_ms;
            snapshot.control.stop_state = SessionStopState::Stopped;
            snapshot.control.stopped_at_ms = Some(now_ms);
            snapshot.last_command_outcome = SessionCommandOutcome {
                command: crate::TOOL_STOP_SESSION.to_owned(),
                observed_at_ms: now_ms,
                disposition: SessionCommandDisposition::Succeeded,
                rejection_code: None,
                message: None,
            };
        })
    }

    pub fn clear_stop(&self, session_id: &str) -> Result<SkillSessionSnapshot, McpContractError> {
        self.mutate(session_id, |snapshot, now_ms| {
            snapshot.revision += 1;
            snapshot.updated_at_ms = now_ms;
            snapshot.control.stop_state = SessionStopState::Active;
            snapshot.control.stopped_at_ms = None;
            snapshot.last_command_outcome = SessionCommandOutcome {
                command: crate::TOOL_CLEAR_STOP.to_owned(),
                observed_at_ms: now_ms,
                disposition: SessionCommandDisposition::Succeeded,
                rejection_code: None,
                message: None,
            };
        })
    }

    pub fn record_command_rejection(
        &self,
        session_id: &str,
        command: &str,
        rejection_code: &str,
        message: impl Into<String>,
    ) -> Result<SkillSessionSnapshot, McpContractError> {
        let message = message.into();
        self.mutate(session_id, |snapshot, now_ms| {
            snapshot.revision += 1;
            snapshot.updated_at_ms = now_ms;
            snapshot.last_command_outcome = SessionCommandOutcome {
                command: command.to_owned(),
                observed_at_ms: now_ms,
                disposition: SessionCommandDisposition::Rejected,
                rejection_code: Some(rejection_code.to_owned()),
                message: Some(message.clone()),
            };
            snapshot.last_rejected_command = Some(SessionCommandRejection {
                command: command.to_owned(),
                observed_at_ms: now_ms,
                rejection_code: rejection_code.to_owned(),
                message: message.clone(),
            });
        })
    }

    fn mutate(
        &self,
        session_id: &str,
        apply: impl FnOnce(&mut SkillSessionSnapshot, u64),
    ) -> Result<SkillSessionSnapshot, McpContractError> {
        let mut sessions = self.sessions.write().expect("session registry write lock");
        let snapshot =
            sessions
                .get_mut(session_id)
                .ok_or_else(|| McpContractError::UnknownSession {
                    session_id: session_id.to_owned(),
                })?;
        let now_ms = unix_timestamp_ms();
        apply(snapshot, now_ms);
        Ok(snapshot.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAction {
    Load,
    Reload,
}

impl SessionAction {
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::Load => crate::TOOL_LOAD_BUNDLE,
            Self::Reload => crate::TOOL_RELOAD_BUNDLE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOperationRecord {
    pub action: SessionAction,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionControlState {
    pub stop_state: SessionStopState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at_ms: Option<u64>,
}

impl Default for SessionControlState {
    fn default() -> Self {
        Self {
            stop_state: SessionStopState::Active,
            stopped_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSessionSnapshot {
    pub session_id: String,
    pub entry_url: String,
    pub session_uri_root: String,
    pub revision: u64,
    pub updated_at_ms: u64,
    pub last_operation: SessionOperationRecord,
    pub control: SessionControlState,
    pub last_command_outcome: SessionCommandOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejected_command: Option<SessionCommandRejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<SessionHandoff>,
    pub lifecycle: BundleLifecycleChange,
    pub outcome: BundleLoadOutcome,
    pub interpretation: SkillBundleInterpretation,
}

impl SkillSessionSnapshot {
    pub fn status(&self) -> SkillBundleInterpretationStatus {
        self.interpretation.status.clone()
    }

    pub fn stop_state(&self) -> SessionStopState {
        self.control.stop_state
    }

    pub fn is_stopped(&self) -> bool {
        self.stop_state() == SessionStopState::Stopped
    }

    pub fn stoppable(&self) -> bool {
        self.stop_state() == SessionStopState::Active
    }

    pub fn clearable(&self) -> bool {
        self.stop_state() == SessionStopState::Stopped
    }

    pub fn blocker_count(&self) -> usize {
        self.interpretation.blockers.len()
    }

    pub fn ambiguity_count(&self) -> usize {
        self.interpretation.ambiguities.len()
    }

    pub fn required_owner_action_count(&self) -> usize {
        self.blocker_count()
            + self.interpretation.setup_requirements.len()
            + self.interpretation.owner_decisions.len()
    }

    pub fn proposal_uri(&self) -> String {
        SkillSessionResourceKind::Proposal.uri_for_session(&self.session_id)
    }

    pub fn proposal_revision(&self) -> u64 {
        self.revision
    }

    pub fn proposal_packet(&self) -> Result<SkillProposalPacket, BundleError> {
        generate_proposal_packet(&self.outcome, &self.interpretation)
    }

    pub fn proposal_readiness(&self) -> Result<SessionProposalReadiness, BundleError> {
        Ok(SessionProposalReadiness::from(
            self.proposal_packet()?.proposal_readiness,
        ))
    }

    pub fn lifecycle_change(&self) -> BundleLifecycleChange {
        self.lifecycle.clone()
    }

    pub fn lifecycle_summary(&self) -> SkillSessionLifecycleSummary {
        let lifecycle = self.lifecycle_change();
        SkillSessionLifecycleSummary {
            classification: lifecycle.classification,
            previous_bundle_version: lifecycle
                .previous
                .as_ref()
                .and_then(|snapshot| snapshot.bundle_version.clone()),
            current_bundle_version: lifecycle
                .current
                .as_ref()
                .and_then(|snapshot| snapshot.bundle_version.clone()),
            previous_compatible_daemon: lifecycle
                .previous
                .as_ref()
                .and_then(|snapshot| snapshot.compatible_daemon.clone()),
            current_compatible_daemon: lifecycle
                .current
                .as_ref()
                .and_then(|snapshot| snapshot.compatible_daemon.clone()),
            changed_documents: lifecycle.changed_documents.clone(),
            diagnostics: lifecycle.diagnostics.clone(),
        }
    }

    pub fn next_operator_step(&self) -> Result<SessionNextOperatorStep, BundleError> {
        if self.is_stopped() {
            return Ok(SessionNextOperatorStep {
                kind: SessionNextOperatorStepKind::ClearStop,
                summary: "Clear the session stop before retrying intake or proposal actions."
                    .to_owned(),
                document_id: None,
                resource_uri: Some(
                    SkillSessionResourceKind::OperatorState.uri_for_session(&self.session_id),
                ),
            });
        }

        if let Some(blocker) = self.interpretation.blockers.first() {
            return Ok(next_step_from_blocker(blocker));
        }

        if let Some(ambiguity) = self.interpretation.ambiguities.first() {
            return Ok(SessionNextOperatorStep {
                kind: SessionNextOperatorStepKind::ResolveAmbiguity,
                summary: ambiguity.summary.clone(),
                document_id: ambiguity
                    .evidence
                    .first()
                    .map(|item| item.document_id.clone()),
                resource_uri: Some(
                    SkillSessionResourceKind::Ambiguities.uri_for_session(&self.session_id),
                ),
            });
        }

        if let Some(requirement) = self.interpretation.setup_requirements.first() {
            return Ok(SessionNextOperatorStep {
                kind: SessionNextOperatorStepKind::SatisfySetupRequirement,
                summary: requirement.summary.clone().unwrap_or_else(|| {
                    format!("Provide setup requirement {}", requirement.requirement_key)
                }),
                document_id: requirement
                    .evidence
                    .first()
                    .map(|item| item.document_id.clone()),
                resource_uri: Some(
                    SkillSessionResourceKind::Interpretation.uri_for_session(&self.session_id),
                ),
            });
        }

        if let Some(decision) = self.interpretation.owner_decisions.first() {
            return Ok(SessionNextOperatorStep {
                kind: SessionNextOperatorStepKind::MakeOwnerDecision,
                summary: decision.decision_text.clone(),
                document_id: decision
                    .evidence
                    .first()
                    .map(|item| item.document_id.clone()),
                resource_uri: Some(
                    SkillSessionResourceKind::Interpretation.uri_for_session(&self.session_id),
                ),
            });
        }

        if let Some(diagnostic) = self.lifecycle.diagnostics.first() {
            return Ok(SessionNextOperatorStep {
                kind: SessionNextOperatorStepKind::InspectLifecycleDiagnostics,
                summary: diagnostic.message.clone(),
                document_id: diagnostic.document_id.clone(),
                resource_uri: Some(
                    SkillSessionResourceKind::Lifecycle.uri_for_session(&self.session_id),
                ),
            });
        }

        let proposal_packet = self.proposal_packet()?;
        let proposal_readiness = SessionProposalReadiness::from(proposal_packet.proposal_readiness);
        let (kind, summary) = match proposal_readiness {
            SessionProposalReadiness::Ready => (
                SessionNextOperatorStepKind::GenerateProposalPacket,
                format!("Generate proposal packet from {}", self.proposal_uri()),
            ),
            SessionProposalReadiness::Incomplete => (
                SessionNextOperatorStepKind::ReviewProposalIncompleteness,
                "Review proposal incompleteness before acting on the bundle.".to_owned(),
            ),
            SessionProposalReadiness::Blocked => (
                SessionNextOperatorStepKind::ResolveBlocker,
                "Resolve blocking issues before generating a proposal packet.".to_owned(),
            ),
        };

        Ok(SessionNextOperatorStep {
            kind,
            summary,
            document_id: None,
            resource_uri: Some(self.proposal_uri()),
        })
    }

    pub fn current_failures(&self) -> Result<Vec<SessionFailureEvidence>, BundleError> {
        let mut failures = Vec::new();

        failures.extend(
            self.interpretation
                .blockers
                .iter()
                .map(SessionFailureEvidence::from_blocker),
        );
        failures.extend(
            self.interpretation
                .ambiguities
                .iter()
                .map(SessionFailureEvidence::from_ambiguity),
        );
        failures.extend(
            self.interpretation
                .setup_requirements
                .iter()
                .map(SessionFailureEvidence::from_setup_requirement),
        );
        failures.extend(
            self.interpretation
                .owner_decisions
                .iter()
                .map(SessionFailureEvidence::from_owner_decision),
        );
        failures.extend(
            self.lifecycle
                .diagnostics
                .iter()
                .map(SessionFailureEvidence::from_lifecycle_diagnostic),
        );

        let proposal = self.proposal_packet()?;
        if proposal.proposal_readiness == a2ex_skill_bundle::ProposalReadiness::Incomplete {
            failures.extend(proposal_profile_failures(
                &proposal.capital_profile.completeness,
                "capital_profile_incomplete",
                &proposal.capital_profile.reason,
                &proposal.capital_profile.evidence,
            ));
            failures.extend(proposal_profile_failures(
                &proposal.cost_profile.completeness,
                "cost_profile_incomplete",
                &proposal.cost_profile.reason,
                &proposal.cost_profile.evidence,
            ));
        }

        Ok(failures)
    }

    pub fn operator_state_resource(
        &self,
    ) -> Result<SkillSessionOperatorStateResource, BundleError> {
        Ok(SkillSessionOperatorStateResource {
            session_id: self.session_id.clone(),
            entry_url: self.entry_url.clone(),
            session_uri_root: self.session_uri_root.clone(),
            revision: self.revision,
            updated_at_ms: self.updated_at_ms,
            stop_state: self.stop_state(),
            stoppable: self.stoppable(),
            clearable: self.clearable(),
            stopped_at_ms: self.control.stopped_at_ms,
            blocker_count: self.blocker_count(),
            ambiguity_count: self.ambiguity_count(),
            required_owner_action_count: self.required_owner_action_count(),
            proposal_readiness: self.proposal_readiness()?,
            lifecycle_classification: self.lifecycle.classification,
            lifecycle_diagnostic_count: self.lifecycle.diagnostics.len(),
            next_operator_step: self.next_operator_step()?,
            last_command_outcome: self.last_command_outcome.clone(),
        })
    }

    pub fn failures_resource(&self) -> Result<SkillSessionFailuresResource, BundleError> {
        Ok(SkillSessionFailuresResource {
            session_id: self.session_id.clone(),
            entry_url: self.entry_url.clone(),
            session_uri_root: self.session_uri_root.clone(),
            revision: self.revision,
            updated_at_ms: self.updated_at_ms,
            stop_state: self.stop_state(),
            stopped_at_ms: self.control.stopped_at_ms,
            blocker_count: self.blocker_count(),
            ambiguity_count: self.ambiguity_count(),
            required_owner_action_count: self.required_owner_action_count(),
            lifecycle_diagnostic_count: self.lifecycle.diagnostics.len(),
            current_failures: self.current_failures()?,
            last_command_outcome: self.last_command_outcome.clone(),
            last_rejected_command: self.last_rejected_command.clone(),
        })
    }

    pub fn resource_payload(
        &self,
        resource: SkillSessionResourceKind,
    ) -> Result<Value, BundleError> {
        match resource {
            SkillSessionResourceKind::Status => {
                let proposal = self.proposal_packet()?;
                let lifecycle = self.lifecycle_summary();
                let mut payload = json!({
                    "session_id": self.session_id.clone(),
                    "entry_url": self.entry_url.clone(),
                    "session_uri_root": self.session_uri_root.clone(),
                    "revision": self.revision,
                    "updated_at_ms": self.updated_at_ms,
                    "last_operation": self.last_operation.clone(),
                    "status": self.status(),
                    "blocker_count": self.blocker_count(),
                    "ambiguity_count": self.ambiguity_count(),
                    "has_bundle": self.outcome.bundle.is_some(),
                    "diagnostic_count": self.outcome.diagnostics.len(),
                    "proposal_uri": self.proposal_uri(),
                    "proposal_revision": self.proposal_revision(),
                    "proposal_readiness": SessionProposalReadiness::from(proposal.proposal_readiness),
                    "capital_profile_completeness": SessionProposalCompleteness::from(proposal.capital_profile.completeness),
                    "cost_profile_completeness": SessionProposalCompleteness::from(proposal.cost_profile.completeness),
                    "lifecycle": SkillSessionStatusResource {
                        session_id: self.session_id.clone(),
                        entry_url: self.entry_url.clone(),
                        session_uri_root: self.session_uri_root.clone(),
                        revision: self.revision,
                        lifecycle,
                    }.lifecycle,
                });
                if let Some(handoff) = &self.handoff {
                    payload
                        .as_object_mut()
                        .expect("status payload object")
                        .insert(
                            "handoff".to_owned(),
                            serde_json::to_value(handoff).expect("handoff serializes"),
                        );
                }
                Ok(payload)
            }
            SkillSessionResourceKind::Bundle => {
                Ok(serde_json::to_value(SkillSessionBundleResource {
                    session_id: self.session_id.clone(),
                    entry_url: self.entry_url.clone(),
                    session_uri_root: self.session_uri_root.clone(),
                    revision: self.revision,
                    bundle: self.outcome.bundle.clone(),
                    diagnostics: self.outcome.diagnostics.clone(),
                    lifecycle: self.lifecycle_summary(),
                })
                .expect("bundle payload serializes"))
            }
            SkillSessionResourceKind::Interpretation => Ok(json!({
                "session_id": self.session_id.clone(),
                "entry_url": self.entry_url.clone(),
                "revision": self.revision,
                "status": self.status(),
                "plan_summary": self.interpretation.plan_summary.clone(),
                "owner_decisions": self.interpretation.owner_decisions.clone(),
                "setup_requirements": self.interpretation.setup_requirements.clone(),
                "automation_boundaries": self.interpretation.automation_boundaries.clone(),
                "risks": self.interpretation.risks.clone(),
                "ambiguities": self.interpretation.ambiguities.clone(),
                "blockers": self.interpretation.blockers.clone(),
                "provenance": self.interpretation.provenance.clone(),
            })),
            SkillSessionResourceKind::Blockers => Ok(json!({
                "session_id": self.session_id.clone(),
                "entry_url": self.entry_url.clone(),
                "revision": self.revision,
                "blocker_count": self.blocker_count(),
                "blockers": self.interpretation.blockers.clone(),
            })),
            SkillSessionResourceKind::Ambiguities => Ok(json!({
                "session_id": self.session_id.clone(),
                "entry_url": self.entry_url.clone(),
                "revision": self.revision,
                "ambiguity_count": self.ambiguity_count(),
                "ambiguities": self.interpretation.ambiguities.clone(),
            })),
            SkillSessionResourceKind::Provenance => Ok(json!({
                "session_id": self.session_id.clone(),
                "entry_url": self.entry_url.clone(),
                "revision": self.revision,
                "provenance": self.interpretation.provenance.clone(),
                "diagnostics": self.outcome.diagnostics.clone(),
            })),
            SkillSessionResourceKind::Lifecycle => {
                Ok(serde_json::to_value(SkillSessionLifecycleResource {
                    session_id: self.session_id.clone(),
                    entry_url: self.entry_url.clone(),
                    session_uri_root: self.session_uri_root.clone(),
                    revision: self.revision,
                    lifecycle: self.lifecycle_change(),
                })
                .expect("lifecycle payload serializes"))
            }
            SkillSessionResourceKind::Proposal => {
                let proposal = self.proposal_packet()?;
                let mut payload = json!({
                    "session_id": self.session_id.clone(),
                    "entry_url": self.entry_url.clone(),
                    "session_uri_root": self.session_uri_root.clone(),
                    "revision": self.revision,
                    "proposal_revision": self.proposal_revision(),
                    "proposal_uri": self.proposal_uri(),
                    "proposal_readiness": SessionProposalReadiness::from(proposal.proposal_readiness.clone()),
                    "capital_profile_completeness": SessionProposalCompleteness::from(proposal.capital_profile.completeness.clone()),
                    "cost_profile_completeness": SessionProposalCompleteness::from(proposal.cost_profile.completeness.clone()),
                    "proposal": proposal,
                });
                if let Some(handoff) = &self.handoff {
                    payload
                        .as_object_mut()
                        .expect("proposal payload object")
                        .insert(
                            "handoff".to_owned(),
                            serde_json::to_value(handoff).expect("handoff serializes"),
                        );
                }
                Ok(payload)
            }
            SkillSessionResourceKind::OperatorState => {
                serde_json::to_value(self.operator_state_resource()?).map_err(|error| {
                    BundleError::NotImplemented {
                        message: format!("failed to serialize operator_state payload: {error}"),
                    }
                })
            }
            SkillSessionResourceKind::Failures => serde_json::to_value(self.failures_resource()?)
                .map_err(|error| BundleError::NotImplemented {
                    message: format!("failed to serialize failures payload: {error}"),
                }),
        }
    }
}

fn next_step_from_blocker(blocker: &InterpretationBlocker) -> SessionNextOperatorStep {
    let kind = match blocker.diagnostic_code {
        Some(BundleDiagnosticCode::MissingRequiredDocument) => {
            SessionNextOperatorStepKind::SupplyRequiredDocument
        }
        _ => SessionNextOperatorStepKind::ResolveBlocker,
    };

    SessionNextOperatorStep {
        kind,
        summary: blocker.summary.clone(),
        document_id: blocker
            .evidence
            .first()
            .map(|item| item.document_id.clone()),
        resource_uri: None,
    }
}

fn proposal_profile_failures(
    completeness: &ProposalQuantitativeCompleteness,
    diagnostic_code: &str,
    reason: &str,
    evidence: &[InterpretationEvidence],
) -> Vec<SessionFailureEvidence> {
    match completeness {
        ProposalQuantitativeCompleteness::Unknown
        | ProposalQuantitativeCompleteness::RequiresOwnerInput => vec![SessionFailureEvidence {
            kind: SessionFailureKind::ProposalIncomplete,
            summary: reason.to_owned(),
            diagnostic_code: Some(diagnostic_code.to_owned()),
            owner_action_required: true,
            document_id: evidence.first().map(|item| item.document_id.clone()),
            evidence: evidence.to_vec(),
        }],
        _ => Vec::new(),
    }
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_millis()
        .try_into()
        .expect("timestamp fits in u64")
}

impl SessionFailureEvidence {
    fn from_blocker(blocker: &InterpretationBlocker) -> Self {
        Self {
            kind: SessionFailureKind::Blocker,
            summary: blocker.summary.clone(),
            diagnostic_code: blocker
                .diagnostic_code
                .as_ref()
                .map(diagnostic_code_to_string),
            owner_action_required: true,
            document_id: blocker
                .evidence
                .first()
                .map(|item| item.document_id.clone()),
            evidence: blocker.evidence.clone(),
        }
    }

    fn from_ambiguity(ambiguity: &InterpretationAmbiguity) -> Self {
        Self {
            kind: SessionFailureKind::Ambiguity,
            summary: ambiguity.summary.clone(),
            diagnostic_code: Some("interpretation_ambiguity".to_owned()),
            owner_action_required: true,
            document_id: ambiguity
                .evidence
                .first()
                .map(|item| item.document_id.clone()),
            evidence: ambiguity.evidence.clone(),
        }
    }

    fn from_setup_requirement(requirement: &InterpretationSetupRequirement) -> Self {
        Self {
            kind: SessionFailureKind::SetupRequirement,
            summary: requirement
                .summary
                .clone()
                .unwrap_or_else(|| requirement.requirement_key.clone()),
            diagnostic_code: Some("setup_requirement".to_owned()),
            owner_action_required: true,
            document_id: requirement
                .evidence
                .first()
                .map(|item| item.document_id.clone()),
            evidence: requirement.evidence.clone(),
        }
    }

    fn from_owner_decision(decision: &InterpretationOwnerDecision) -> Self {
        Self {
            kind: SessionFailureKind::OwnerDecision,
            summary: decision.decision_text.clone(),
            diagnostic_code: Some("owner_decision_required".to_owned()),
            owner_action_required: true,
            document_id: decision
                .evidence
                .first()
                .map(|item| item.document_id.clone()),
            evidence: decision.evidence.clone(),
        }
    }

    fn from_lifecycle_diagnostic(
        diagnostic: &a2ex_skill_bundle::BundleLifecycleDiagnostic,
    ) -> Self {
        Self {
            kind: SessionFailureKind::LifecycleDiagnostic,
            summary: diagnostic.message.clone(),
            diagnostic_code: Some(lifecycle_diagnostic_code_to_string(&diagnostic.code)),
            owner_action_required: true,
            document_id: diagnostic.document_id.clone(),
            evidence: Vec::new(),
        }
    }
}

fn diagnostic_code_to_string(code: &BundleDiagnosticCode) -> String {
    serde_json::to_string(code)
        .expect("diagnostic code serializes")
        .trim_matches('"')
        .to_owned()
}

fn lifecycle_diagnostic_code_to_string(
    code: &a2ex_skill_bundle::BundleLifecycleDiagnosticCode,
) -> String {
    serde_json::to_string(code)
        .expect("lifecycle diagnostic code serializes")
        .trim_matches('"')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use a2ex_skill_bundle::{
        BundleDiagnostic, BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity,
        BundleLoadOutcome, InterpretationBlocker, InterpretationEvidence,
        InterpretationPlanSummary, SkillBundleInterpretation, SkillBundleInterpretationStatus,
    };
    use url::Url;

    use super::*;

    #[test]
    fn registry_reuses_session_identity_and_increments_revision() {
        let registry = SkillSessionRegistry::default();
        let first = registry.upsert(
            SessionAction::Load,
            "https://bundles.a2ex.local/skill.md".to_owned(),
            sample_outcome(),
            sample_interpretation(),
            None,
        );
        let second = registry.upsert(
            SessionAction::Reload,
            "https://bundles.a2ex.local/skill.md".to_owned(),
            sample_outcome(),
            sample_interpretation(),
            None,
        );

        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 2);
        assert_eq!(second.last_operation.action, SessionAction::Reload);
        assert_eq!(
            second.last_command_outcome.command,
            crate::TOOL_RELOAD_BUNDLE
        );
    }

    #[test]
    fn status_resource_exposes_observable_session_fields() {
        let snapshot = SkillSessionRegistry::default().upsert(
            SessionAction::Load,
            "https://bundles.a2ex.local/skill.md".to_owned(),
            sample_outcome(),
            sample_interpretation(),
            None,
        );

        let status = snapshot
            .resource_payload(SkillSessionResourceKind::Status)
            .expect("status payload builds");
        assert_eq!(status["status"], "blocked");
        assert_eq!(status["blocker_count"], 1);
        assert_eq!(status["has_bundle"], false);
        assert_eq!(status["diagnostic_count"], 1);
        assert_eq!(status["proposal_uri"], snapshot.proposal_uri());
        assert_eq!(status["proposal_readiness"], "blocked");
        assert_eq!(status["capital_profile_completeness"], "blocked");
    }

    #[test]
    fn operator_and_failure_resources_preserve_stop_and_rejection_state() {
        let registry = SkillSessionRegistry::default();
        let loaded = registry.upsert(
            SessionAction::Load,
            "https://bundles.a2ex.local/skill.md".to_owned(),
            sample_outcome(),
            sample_interpretation(),
            None,
        );
        let stopped = registry
            .stop_session(&loaded.session_id)
            .expect("stop works");
        let rejected = registry
            .record_command_rejection(
                &loaded.session_id,
                crate::TOOL_GENERATE_PROPOSAL_PACKET,
                "session_stopped",
                "session is stopped",
            )
            .expect("rejection persists");

        let operator_state = stopped.operator_state_resource().expect("resource builds");
        assert_eq!(operator_state.stop_state, SessionStopState::Stopped);
        assert!(!operator_state.stoppable);
        assert!(operator_state.clearable);
        assert_eq!(
            operator_state.last_command_outcome.command,
            crate::TOOL_STOP_SESSION
        );
        assert_eq!(
            operator_state.next_operator_step.kind,
            SessionNextOperatorStepKind::ClearStop
        );

        let failures = rejected.failures_resource().expect("resource builds");
        assert_eq!(failures.stop_state, SessionStopState::Stopped);
        assert_eq!(failures.blocker_count, 1);
        assert_eq!(
            failures.current_failures[0].diagnostic_code.as_deref(),
            Some("missing_required_document")
        );
        assert_eq!(
            failures
                .last_rejected_command
                .as_ref()
                .map(|record| record.rejection_code.as_str()),
            Some("session_stopped")
        );
    }

    #[test]
    fn proposal_resource_tracks_current_revision() {
        let snapshot = SkillSessionRegistry::default().upsert(
            SessionAction::Load,
            "https://bundles.a2ex.local/skill.md".to_owned(),
            ready_outcome(),
            ready_interpretation(),
            None,
        );

        let proposal = snapshot
            .resource_payload(SkillSessionResourceKind::Proposal)
            .expect("proposal payload builds");
        assert_eq!(proposal["proposal_revision"], snapshot.revision);
        assert_eq!(proposal["proposal_uri"], snapshot.proposal_uri());
        assert_eq!(proposal["proposal"]["proposal_readiness"], "ready");
    }

    fn sample_outcome() -> BundleLoadOutcome {
        BundleLoadOutcome {
            bundle: None,
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::MissingRequiredDocument,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::LoadManifest,
                message: "required document missing".to_owned(),
                document_id: Some("owner-setup".to_owned()),
                source_url: Some(
                    Url::parse("https://bundles.a2ex.local/docs/owner-setup.md")
                        .expect("url parses"),
                ),
                section_slug: None,
            }],
        }
    }

    fn sample_interpretation() -> SkillBundleInterpretation {
        SkillBundleInterpretation {
            status: SkillBundleInterpretationStatus::Blocked,
            plan_summary: None,
            owner_decisions: Vec::new(),
            setup_requirements: Vec::new(),
            automation_boundaries: Vec::new(),
            risks: Vec::new(),
            ambiguities: Vec::new(),
            blockers: vec![InterpretationBlocker {
                blocker_key: "owner-setup:missing_required_document".to_owned(),
                summary: "required document missing".to_owned(),
                diagnostic_code: Some(BundleDiagnosticCode::MissingRequiredDocument),
                diagnostic_severity: Some(BundleDiagnosticSeverity::Error),
                diagnostic_phase: Some(BundleDiagnosticPhase::LoadManifest),
                evidence: vec![InterpretationEvidence {
                    document_id: "owner-setup".to_owned(),
                    section_id: None,
                    section_slug: None,
                    source_url: Url::parse("https://bundles.a2ex.local/docs/owner-setup.md")
                        .expect("url parses"),
                }],
            }],
            provenance: Vec::new(),
        }
    }

    fn ready_outcome() -> BundleLoadOutcome {
        BundleLoadOutcome {
            bundle: None,
            diagnostics: Vec::new(),
        }
    }

    fn ready_interpretation() -> SkillBundleInterpretation {
        SkillBundleInterpretation {
            status: SkillBundleInterpretationStatus::InterpretedReady,
            plan_summary: Some(InterpretationPlanSummary {
                bundle_id: "official.demo".to_owned(),
                title: Some("Demo".to_owned()),
                summary: Some("Summary".to_owned()),
                overview: Some("Overview".to_owned()),
                evidence: vec![InterpretationEvidence {
                    document_id: "skill".to_owned(),
                    section_id: Some("skill#overview".to_owned()),
                    section_slug: Some("overview".to_owned()),
                    source_url: Url::parse("https://bundles.a2ex.local/skill.md")
                        .expect("url parses"),
                }],
            }),
            owner_decisions: Vec::new(),
            setup_requirements: Vec::new(),
            automation_boundaries: Vec::new(),
            risks: Vec::new(),
            ambiguities: Vec::new(),
            blockers: Vec::new(),
            provenance: Vec::new(),
        }
    }
}
