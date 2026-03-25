use std::{fs, io, path::Path};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    a2ex_rereads::A2exCanonicalRereads,
    assembly_summary::{
        ASSEMBLY_SUMMARY_FILE_NAME, AssemblyFailureDiagnostic, AssemblyPhase, AssemblySummaryRefs,
        AssemblyVerificationStatus, DecisiveSnapshot, PostControlVerification,
    },
    evidence_bundle::{CanonicalRereadRefs, EvidenceBundleRefs},
    live_route::{LiveRouteContract, s02_live_route_contract},
    preflight::PreflightReport,
    runtime_metadata::RuntimeMetadata,
    verdict::{
        ClassifiedVerdict, LiveRouteVerdictState, VerdictClassifierInput,
        VerdictMismatchDiagnostic, classify_verdict,
    },
    waiaas::{
        DEFAULT_AUTHORITY_EVIDENCE_REF, DEFAULT_WALLET_BOUNDARY, WaiaasAuthorityCapture,
        WaiaasAuthorityOutcome,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessRunState {
    pub run_id: String,
    pub install_id: Option<String>,
    pub proposal_id: Option<String>,
    pub selection_id: Option<String>,
    pub waiaas: WaiaasInspection,
    #[serde(default)]
    pub live_route: LiveRouteContract,
    #[serde(default)]
    pub waiaas_authority: WaiaasAuthorityState,
    #[serde(default)]
    pub live_attempt: LiveAttemptState,
    #[serde(default)]
    pub evidence_refs: EvidenceRefs,
    #[serde(default)]
    pub evidence_bundle: EvidenceBundleRefs,
    // S04 persists assembly-summary.json alongside run-state.json so the decisive snapshot
    // and later post_control_verification remain readable under one run_id.
    #[serde(default)]
    pub assembly_summary: AssemblySummaryRefs,
    #[serde(default)]
    pub assembly_phase: AssemblyPhase,
    #[serde(default)]
    pub decisive_snapshot: DecisiveSnapshot,
    #[serde(default)]
    pub post_control_verification: PostControlVerification,
    #[serde(default)]
    pub canonical_rereads: CanonicalRereadRefs,
    #[serde(default)]
    pub canonical_reread_collection: A2exCanonicalRereads,
    #[serde(default)]
    pub live_route_result: LiveRouteVerdictState,
    #[serde(default)]
    pub final_classification: FinalClassification,
    pub last_phase: HarnessPhase,
    pub phase_history: Vec<PhaseTransition>,
    pub runtime_metadata: RuntimeMetadata,
    pub launch: LaunchMetadata,
    pub reconnect: ReconnectStatus,
    pub canonical: CanonicalCorrelation,
    pub last_error_class: Option<String>,
    pub pre_trade_outcome: PreTradeOutcome,
    pub preflight: PreflightReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessPhase {
    Preflight,
    OpenclawLaunchPending,
    OpenclawAttached,
    InstallBootstrapPending,
    ProposalGenerationPending,
    RouteReadinessPending,
    ApprovalPending,
    ApprovalBoundaryReached,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseTransition {
    pub phase: HarnessPhase,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LaunchMetadata {
    pub openclaw_pid: Option<u32>,
    pub openclaw_mode: Option<String>,
    pub openclaw_request_path: Option<String>,
    pub openclaw_guidance_path: Option<String>,
    pub openclaw_stdout_path: Option<String>,
    pub openclaw_stderr_path: Option<String>,
    pub mcp_spawn_command: Vec<String>,
    pub run_state_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaiaasInspection {
    pub base_url: Option<String>,
    pub health_url: Option<String>,
    pub session_url: Option<String>,
    pub policy_url: Option<String>,
    pub session_id: Option<String>,
    pub policy_id: Option<String>,
}

impl Default for WaiaasInspection {
    fn default() -> Self {
        Self {
            base_url: None,
            health_url: None,
            session_url: None,
            policy_url: None,
            session_id: None,
            policy_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaiaasAuthorityState {
    pub session_id: Option<String>,
    pub policy_id: Option<String>,
    pub authority_decision: AuthorityDecision,
    pub reason_code: String,
    pub authority_timestamp: Option<String>,
    pub wallet_boundary: String,
    pub evidence_ref: Option<String>,
}

impl Default for WaiaasAuthorityState {
    fn default() -> Self {
        Self {
            session_id: None,
            policy_id: None,
            authority_decision: AuthorityDecision::Hold,
            reason_code: "awaiting_waiaas_authority_check".to_owned(),
            authority_timestamp: None,
            wallet_boundary: DEFAULT_WALLET_BOUNDARY.to_owned(),
            evidence_ref: Some(DEFAULT_AUTHORITY_EVIDENCE_REF.to_owned()),
        }
    }
}

impl From<&WaiaasAuthorityCapture> for WaiaasAuthorityState {
    fn from(capture: &WaiaasAuthorityCapture) -> Self {
        Self {
            session_id: capture.session_id.clone(),
            policy_id: capture.policy_id.clone(),
            authority_decision: capture.authority_decision,
            reason_code: capture.reason_code.clone(),
            authority_timestamp: capture.authority_timestamp.clone(),
            wallet_boundary: capture.wallet_boundary.clone(),
            evidence_ref: Some(capture.evidence_ref.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// Frozen S02 authority vocabulary: pass / fail / hold / blocked.
pub enum AuthorityDecision {
    Pass,
    Fail,
    Hold,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveAttemptState {
    pub attempt_phase: LiveAttemptPhase,
    pub phase_history: Vec<LiveAttemptTransition>,
}

impl Default for LiveAttemptState {
    fn default() -> Self {
        Self {
            attempt_phase: LiveAttemptPhase::ApprovalBoundaryPending,
            phase_history: vec![LiveAttemptTransition {
                attempt_phase: LiveAttemptPhase::ApprovalBoundaryPending,
                detail: "bounded live route not started; waiting for the approval boundary"
                    .to_owned(),
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveAttemptTransition {
    pub attempt_phase: LiveAttemptPhase,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveAttemptPhase {
    ApprovalBoundaryPending,
    WaiaasAuthorityPending,
    RouteExecutionPending,
    DestinationConfirmationPending,
    Completed,
    Hold,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRefs {
    pub waiaas_authority: EvidenceRef,
    pub live_route_evidence: EvidenceRef,
}

impl Default for EvidenceRefs {
    fn default() -> Self {
        Self {
            waiaas_authority: EvidenceRef {
                evidence_ref: Some(DEFAULT_AUTHORITY_EVIDENCE_REF.to_owned()),
                description:
                    "typed WAIaaS authority evidence for session_id, policy_id, authority_decision, reason_code, and wallet_boundary"
                        .to_owned(),
            },
            live_route_evidence: EvidenceRef {
                evidence_ref: Some("live-route-evidence.json".to_owned()),
                description:
                    "typed live route evidence for Across destination-chain USDC receipt on Base"
                        .to_owned(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub evidence_ref: Option<String>,
    pub description: String,
}

// S03 verdict classifier fields stay reconnect-safe even before the dedicated classifier module lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalClassification {
    pub verdict: Verdict,
    pub reason_code: String,
    pub decisive_evidence_ref: Option<String>,
    pub decisive_evidence_refs: Vec<String>,
    pub summary: String,
    pub reasoning_summary: String,
    pub reasoning_evidence_refs: Vec<String>,
    pub reread_snapshot_refs: Vec<String>,
    pub regenerated_from_persisted_facts: bool,
    #[serde(default)]
    pub mismatch_diagnostics: Vec<VerdictMismatchDiagnostic>,
}

impl Default for FinalClassification {
    fn default() -> Self {
        Self {
            verdict: Verdict::Hold,
            reason_code: "awaiting_live_route_execution".to_owned(),
            decisive_evidence_ref: Some("live-route-evidence.json".to_owned()),
            decisive_evidence_refs: vec!["live-route-evidence.json".to_owned()],
            summary: "approval or mutation receipts alone cannot mark the bounded live route green"
                .to_owned(),
            reasoning_summary:
                "approval or mutation receipts alone cannot mark the bounded live route green"
                    .to_owned(),
            reasoning_evidence_refs: vec!["live-route-evidence.json".to_owned()],
            reread_snapshot_refs: CanonicalRereadRefs::default().all_refs(),
            regenerated_from_persisted_facts: false,
            mismatch_diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// Frozen S02 verdict vocabulary: pass / fail / hold / blocked.
pub enum Verdict {
    Pass,
    Fail,
    Hold,
    Blocked,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Hold => "hold",
            Self::Blocked => "blocked",
        }
    }
}

impl Default for LiveRouteContract {
    fn default() -> Self {
        s02_live_route_contract()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectStatus {
    pub requires_bootstrap_install: bool,
    pub install_locator_status: InstallLocatorStatus,
    pub bootstrap_tool: String,
    pub guidance: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallLocatorStatus {
    RepopulationRequired,
    PendingCanonicalInstall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalCorrelation {
    pub install_url: Option<String>,
    pub state_db_path: Option<String>,
    pub correlation_source: Option<String>,
    pub install_id: Option<String>,
    pub proposal_id: Option<String>,
    pub selection_id: Option<String>,
    pub onboarding_status: Option<String>,
    pub readiness_status: Option<String>,
    pub selection_status: Option<String>,
}

impl Default for CanonicalCorrelation {
    fn default() -> Self {
        Self {
            install_url: None,
            state_db_path: None,
            correlation_source: None,
            install_id: None,
            proposal_id: None,
            selection_id: None,
            onboarding_status: None,
            readiness_status: None,
            selection_status: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreTradeOutcome {
    Pending,
    Blocked,
    Incomplete,
    ApprovalBoundaryReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalStateReread {
    pub install_url: String,
    pub state_db_path: String,
    pub install_id: Option<String>,
    pub proposal_id: Option<String>,
    pub selection_id: Option<String>,
    pub onboarding_status: Option<String>,
    pub readiness_status: Option<String>,
    pub selection_status: Option<String>,
}

impl CanonicalStateReread {
    pub fn pre_trade_outcome(&self) -> PreTradeOutcome {
        if self.selection_status.as_deref() == Some("approved") {
            return PreTradeOutcome::ApprovalBoundaryReached;
        }

        if self.selection_status.as_deref() == Some("blocked")
            || self.readiness_status.as_deref() == Some("blocked")
            || self.onboarding_status.as_deref() == Some("blocked")
        {
            return PreTradeOutcome::Blocked;
        }

        PreTradeOutcome::Incomplete
    }
}

impl HarnessRunState {
    pub fn new(preflight: PreflightReport) -> Self {
        let last_error_class = preflight
            .failures
            .first()
            .map(|issue| issue.class.to_string());
        let runtime_metadata = preflight.runtime_metadata.clone();
        let canonical_rereads = CanonicalRereadRefs::default();
        Self {
            run_id: Uuid::now_v7().to_string(),
            install_id: None,
            proposal_id: None,
            selection_id: None,
            waiaas: WaiaasInspection::default(),
            live_route: s02_live_route_contract(),
            waiaas_authority: WaiaasAuthorityState::default(),
            live_attempt: LiveAttemptState::default(),
            evidence_refs: EvidenceRefs::default(),
            evidence_bundle: EvidenceBundleRefs::for_runtime_metadata(runtime_metadata.clone()),
            assembly_summary: AssemblySummaryRefs::default(),
            assembly_phase: AssemblyPhase::AwaitingDecisiveSnapshot,
            decisive_snapshot: DecisiveSnapshot::default(),
            post_control_verification: PostControlVerification::default(),
            canonical_rereads,
            canonical_reread_collection: A2exCanonicalRereads::default(),
            live_route_result: LiveRouteVerdictState::default(),
            final_classification: FinalClassification::default(),
            last_phase: HarnessPhase::Preflight,
            phase_history: vec![PhaseTransition {
                phase: HarnessPhase::Preflight,
                detail: "typed preflight completed".to_owned(),
            }],
            runtime_metadata,
            launch: LaunchMetadata::default(),
            reconnect: ReconnectStatus {
                requires_bootstrap_install: true,
                install_locator_status: InstallLocatorStatus::PendingCanonicalInstall,
                bootstrap_tool: "onboarding.bootstrap_install".to_owned(),
                guidance: vec![
                    "After restart, rerun onboarding.bootstrap_install with the same install_url to repopulate install locators before canonical rereads.".to_owned(),
                    "Reuse the same run_id when resuming so the reopened install_id, proposal_id, and selection_id stay correlated in this run folder.".to_owned(),
                ],
            },
            canonical: CanonicalCorrelation::default(),
            last_error_class,
            pre_trade_outcome: PreTradeOutcome::Pending,
            preflight,
        }
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn transition(&mut self, phase: HarnessPhase, detail: impl Into<String>) {
        self.last_phase = phase;
        self.phase_history.push(PhaseTransition {
            phase,
            detail: detail.into(),
        });
    }

    pub fn set_live_attempt_phase(
        &mut self,
        attempt_phase: LiveAttemptPhase,
        detail: impl Into<String>,
    ) {
        self.live_attempt.attempt_phase = attempt_phase;
        self.live_attempt.phase_history.push(LiveAttemptTransition {
            attempt_phase,
            detail: detail.into(),
        });
    }

    pub fn apply_classified_verdict(&mut self, classified: ClassifiedVerdict) {
        let mut reasoning_evidence_refs = classified.reasoning_evidence_refs.clone();
        if !reasoning_evidence_refs.contains(&self.evidence_bundle.openclaw_action_summary_ref) {
            reasoning_evidence_refs.push(self.evidence_bundle.openclaw_action_summary_ref.clone());
        }
        self.final_classification = FinalClassification {
            verdict: classified.verdict,
            reason_code: classified.reason_code,
            decisive_evidence_ref: classified.decisive_evidence_ref,
            decisive_evidence_refs: classified.decisive_evidence_refs,
            summary: classified.summary,
            reasoning_summary: classified.reasoning_summary,
            reasoning_evidence_refs,
            reread_snapshot_refs: classified.reread_snapshot_refs,
            regenerated_from_persisted_facts: classified.regenerated_from_persisted_facts,
            mismatch_diagnostics: classified.mismatch_diagnostics,
        };
    }

    pub fn recompute_final_classification(&mut self, regenerated_from_persisted_facts: bool) {
        let classified = classify_verdict(&VerdictClassifierInput {
            canonical_rereads: self.canonical_reread_collection.clone(),
            canonical_reread_refs: self.canonical_rereads.clone(),
            waiaas_authority: self.waiaas_authority.clone(),
            live_route: self.live_route_result.clone(),
            evidence_bundle: self.evidence_bundle.clone(),
            pre_trade_outcome: self.pre_trade_outcome,
            regenerated_from_persisted_facts,
        });
        self.apply_classified_verdict(classified);
    }

    pub fn set_launch_metadata(&mut self, launch: LaunchMetadata) {
        self.launch = launch;
    }

    pub fn set_waiaas(&mut self, waiaas: WaiaasInspection) {
        self.waiaas = waiaas.clone();
        self.waiaas_authority.session_id = waiaas.session_id;
        self.waiaas_authority.policy_id = waiaas.policy_id;
    }

    pub fn apply_waiaas_authority(&mut self, outcome: &WaiaasAuthorityOutcome) {
        let capture = outcome.capture();
        self.waiaas_authority = WaiaasAuthorityState::from(capture);
        self.evidence_refs.waiaas_authority.evidence_ref = Some(capture.evidence_ref.clone());

        match outcome {
            WaiaasAuthorityOutcome::Pass(_) => {
                self.set_live_attempt_phase(
                    LiveAttemptPhase::RouteExecutionPending,
                    "WAIaaS authority pass recorded; the bounded live route may start within the same wallet boundary",
                );
                self.live_route_result.attempt_decision = Verdict::Hold;
                self.live_route_result.reason_code = "awaiting_live_route_execution".to_owned();
                self.live_route_result.summary =
                    "WAIaaS authority is present, but decisive live-route evidence is still required"
                        .to_owned();
            }
            WaiaasAuthorityOutcome::Blocked(_) => {
                self.set_live_attempt_phase(
                    LiveAttemptPhase::Blocked,
                    format!(
                        "WAIaaS authority blocked the bounded live route: {}",
                        capture.reason_code
                    ),
                );
                self.live_route_result.attempt_decision = Verdict::Blocked;
                self.live_route_result.reason_code = capture.reason_code.clone();
                self.live_route_result.summary =
                    "WAIaaS session/policy authority blocked the live attempt before route execution"
                        .to_owned();
            }
            WaiaasAuthorityOutcome::Hold(_) => {
                self.set_live_attempt_phase(
                    LiveAttemptPhase::Hold,
                    format!(
                        "WAIaaS authority is holding the bounded live route: {}",
                        capture.reason_code
                    ),
                );
                self.live_route_result.attempt_decision = Verdict::Hold;
                self.live_route_result.reason_code = capture.reason_code.clone();
                self.live_route_result.summary =
                    "WAIaaS authority reported a recoverable hold before route execution"
                        .to_owned();
            }
            WaiaasAuthorityOutcome::Fail(_) => {
                self.last_error_class = Some("waiaas_authority_failed".to_owned());
                self.set_live_attempt_phase(
                    LiveAttemptPhase::Failed,
                    format!(
                        "WAIaaS authority inspection failed before route execution: {}",
                        capture.reason_code
                    ),
                );
                self.live_route_result.attempt_decision = Verdict::Fail;
                self.live_route_result.reason_code = capture.reason_code.clone();
                self.live_route_result.summary =
                    "WAIaaS authority inspection failed before the bounded live route could start"
                        .to_owned();
            }
        }
        self.recompute_final_classification(false);
    }

    pub fn set_install_url(&mut self, install_url: impl Into<String>) {
        self.canonical.install_url = Some(install_url.into());
        self.refresh_reconnect_guidance();
    }

    pub fn apply_canonical_reread(&mut self, reread: CanonicalStateReread) {
        self.install_id = reread.install_id.clone();
        self.proposal_id = reread.proposal_id.clone();
        self.selection_id = reread.selection_id.clone();
        self.canonical = CanonicalCorrelation {
            install_url: Some(reread.install_url),
            state_db_path: Some(reread.state_db_path),
            correlation_source: Some("state_db_reread".to_owned()),
            install_id: self.install_id.clone(),
            proposal_id: self.proposal_id.clone(),
            selection_id: self.selection_id.clone(),
            onboarding_status: reread.onboarding_status,
            readiness_status: reread.readiness_status,
            selection_status: reread.selection_status,
        };
        self.refresh_canonical_rereads();
        self.pre_trade_outcome = self.canonical_outcome();
        if self.pre_trade_outcome == PreTradeOutcome::ApprovalBoundaryReached {
            self.set_live_attempt_phase(
                LiveAttemptPhase::WaiaasAuthorityPending,
                "canonical approval boundary reached; ready to begin WAIaaS-governed live attempt",
            );
        }
        self.refresh_reconnect_guidance();
        self.recompute_final_classification(false);
    }

    pub fn set_canonical_reread_collection(
        &mut self,
        reread_collection: A2exCanonicalRereads,
        regenerated_from_persisted_facts: bool,
    ) {
        self.canonical_reread_collection = reread_collection;
        self.recompute_final_classification(regenerated_from_persisted_facts);
    }

    pub fn freeze_decisive_snapshot(&mut self, captured_at: impl Into<String>) {
        let captured_at = captured_at.into();
        self.sync_assembly_summary_refs();
        self.decisive_snapshot = DecisiveSnapshot::from_run_state(self, captured_at);
        self.assembly_phase = AssemblyPhase::DecisiveSnapshotFrozen;
        self.post_control_verification.same_run_id = self.run_id.clone();
        self.post_control_verification
            .must_not_overwrite_decisive_verdict = true;
    }

    // snapshot_vs_post_control_mismatch stays separate from the decisive snapshot so
    // runtime.stop and runtime.clear_stop inspection cannot rewrite pass/fail/hold/blocked truth.
    pub fn note_post_control_verification(
        &mut self,
        report_status: AssemblyVerificationStatus,
        runtime_stop_status: AssemblyVerificationStatus,
        runtime_clear_stop_status: AssemblyVerificationStatus,
        mismatch_status: AssemblyVerificationStatus,
        diagnostics: Vec<AssemblyFailureDiagnostic>,
    ) {
        self.post_control_verification.report_status = report_status;
        self.post_control_verification.runtime_stop_status = runtime_stop_status;
        self.post_control_verification.runtime_clear_stop_status = runtime_clear_stop_status;
        self.post_control_verification.mismatch_status = mismatch_status;
        self.post_control_verification
            .last_lifecycle_check_diagnostics = diagnostics;
        self.post_control_verification.same_run_id = self.run_id.clone();
        self.assembly_phase = if report_status == AssemblyVerificationStatus::NotStarted
            && runtime_stop_status == AssemblyVerificationStatus::NotStarted
            && runtime_clear_stop_status == AssemblyVerificationStatus::NotStarted
            && mismatch_status == AssemblyVerificationStatus::NotStarted
        {
            AssemblyPhase::DecisiveSnapshotFrozen
        } else if report_status == AssemblyVerificationStatus::NotStarted
            || runtime_stop_status == AssemblyVerificationStatus::NotStarted
            || runtime_clear_stop_status == AssemblyVerificationStatus::NotStarted
            || mismatch_status == AssemblyVerificationStatus::NotStarted
        {
            AssemblyPhase::PostControlVerificationPending
        } else {
            AssemblyPhase::PostControlVerificationComplete
        };
    }

    pub fn record_post_control_failure(
        &mut self,
        code: impl Into<String>,
        summary: impl Into<String>,
        evidence_refs: Vec<String>,
    ) {
        let diagnostic = AssemblyFailureDiagnostic {
            code: code.into(),
            summary: summary.into(),
            evidence_refs,
            observed_at: now_unix_timestamp(),
        };
        self.post_control_verification
            .last_lifecycle_check_diagnostics
            .push(diagnostic);
        self.post_control_verification.same_run_id = self.run_id.clone();
        self.assembly_phase = AssemblyPhase::PostControlVerificationPending;
    }

    pub fn mark_incomplete(&mut self, detail: impl Into<String>) {
        let detail = detail.into();
        self.pre_trade_outcome = PreTradeOutcome::Incomplete;
        self.transition(self.last_phase, detail.clone());
        self.set_live_attempt_phase(LiveAttemptPhase::Hold, detail);
        self.live_route_result.attempt_decision = Verdict::Hold;
        self.live_route_result.reason_code = "live_route_incomplete".to_owned();
        self.live_route_result.summary =
            "approval boundary may be reached, but decisive live-route evidence is still missing"
                .to_owned();
        self.recompute_final_classification(false);
    }

    pub fn mark_error(&mut self, error_class: impl Into<String>, detail: impl Into<String>) {
        let detail = detail.into();
        self.last_error_class = Some(error_class.into());
        self.transition(HarnessPhase::Failed, detail.clone());
        self.set_live_attempt_phase(LiveAttemptPhase::Failed, detail.clone());
        self.live_route_result.attempt_decision = Verdict::Fail;
        self.live_route_result.reason_code = "harness_error".to_owned();
        self.live_route_result.summary = detail;
        if self.pre_trade_outcome == PreTradeOutcome::Pending {
            self.pre_trade_outcome = PreTradeOutcome::Incomplete;
        }
        self.recompute_final_classification(false);
    }

    pub fn persist(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(self).expect("run state serializes"),
        )
    }

    pub fn canonical_outcome(&self) -> PreTradeOutcome {
        if self.canonical.selection_status.as_deref() == Some("approved") {
            PreTradeOutcome::ApprovalBoundaryReached
        } else if self.canonical.selection_status.as_deref() == Some("blocked")
            || self.canonical.readiness_status.as_deref() == Some("blocked")
            || self.canonical.onboarding_status.as_deref() == Some("blocked")
        {
            PreTradeOutcome::Blocked
        } else if self.install_id.is_some()
            || self.proposal_id.is_some()
            || self.selection_id.is_some()
        {
            PreTradeOutcome::Incomplete
        } else {
            self.pre_trade_outcome
        }
    }

    pub fn refresh_canonical_rereads(&mut self) {
        self.canonical_rereads = CanonicalRereadRefs::for_selection(
            self.install_id.as_deref(),
            self.proposal_id.as_deref(),
            self.selection_id.as_deref(),
        );
        self.live_route_result.evidence_ref =
            self.evidence_refs.live_route_evidence.evidence_ref.clone();
        self.final_classification.reread_snapshot_refs = self.canonical_rereads.all_refs();
        self.sync_assembly_summary_refs();
    }

    pub fn sync_assembly_summary_refs(&mut self) {
        self.assembly_summary.summary_ref = ASSEMBLY_SUMMARY_FILE_NAME.to_owned();
        self.post_control_verification.same_run_id = self.run_id.clone();
        self.decisive_snapshot.run_id = self.run_id.clone();
        self.decisive_snapshot.evidence_bundle_ref = self.evidence_bundle.bundle_ref.clone();
        self.decisive_snapshot.evidence_summary_ref = self.evidence_bundle.summary_ref.clone();
        self.decisive_snapshot.openclaw_action_summary_ref =
            self.evidence_bundle.openclaw_action_summary_ref.clone();
        self.decisive_snapshot.live_route_evidence_ref =
            self.evidence_refs.live_route_evidence.evidence_ref.clone();
        self.decisive_snapshot.waiaas_authority_ref =
            self.evidence_refs.waiaas_authority.evidence_ref.clone();
        self.decisive_snapshot.reread_snapshot_refs = self.canonical_rereads.all_refs();
        self.evidence_bundle.assembly_summary_ref = self.assembly_summary.summary_ref.clone();
    }

    pub fn refresh_reconnect_guidance(&mut self) {
        self.reconnect.install_locator_status = if self.install_id.is_some() {
            InstallLocatorStatus::RepopulationRequired
        } else {
            InstallLocatorStatus::PendingCanonicalInstall
        };

        let install_url = self
            .canonical
            .install_url
            .clone()
            .unwrap_or_else(|| "<same install_url>".to_owned());
        let expected_install_note = match self.install_id.as_deref() {
            Some(install_id) => format!(
                " When reopening, pass expected_install_id={install_id} so canonical install identity stays stable."
            ),
            None => String::new(),
        };
        self.reconnect.guidance = vec![
            format!(
                "After restart, rerun onboarding.bootstrap_install with install_url={install_url} to repopulate install locators before canonical rereads.{expected_install_note}"
            ),
            format!(
                "Preserve run_id={} so the reopened install_id, proposal_id, selection_id, live_route, evidence_refs, and final_classification remain correlated in this run record.",
                self.run_id
            ),
        ];
    }
}

pub fn reread_canonical_state(
    state_db_path: &Path,
    install_url: &str,
) -> io::Result<CanonicalStateReread> {
    let connection = Connection::open(state_db_path)
        .map_err(|error| io::Error::other(format!("state.db open failed: {error}")))?;

    let install_row = connection
        .query_row(
            "SELECT install_id, onboarding_status, readiness_status
             FROM onboarding_installs
             WHERE install_url = ?1
             ORDER BY updated_at DESC, created_at DESC
             LIMIT 1",
            [install_url],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| io::Error::other(format!("install reread failed: {error}")))?;

    let (install_id, onboarding_status, readiness_status) = match install_row {
        Some(row) => row,
        None => {
            return Ok(CanonicalStateReread {
                install_url: install_url.to_owned(),
                state_db_path: state_db_path.display().to_string(),
                install_id: None,
                proposal_id: None,
                selection_id: None,
                onboarding_status: None,
                readiness_status: None,
                selection_status: None,
            });
        }
    };

    let selection_row = connection
        .query_row(
            "SELECT proposal_id, selection_id, status
             FROM onboarding_strategy_selections
             WHERE install_id = ?1
             ORDER BY updated_at DESC, created_at DESC
             LIMIT 1",
            [install_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| io::Error::other(format!("selection reread failed: {error}")))?;

    let route_proposal_id = connection
        .query_row(
            "SELECT proposal_id
             FROM onboarding_route_readiness
             WHERE install_id = ?1
             ORDER BY updated_at DESC, created_at DESC
             LIMIT 1",
            [install_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| io::Error::other(format!("route reread failed: {error}")))?;

    let (proposal_id, selection_id, selection_status) = match selection_row {
        Some((proposal_id, selection_id, selection_status)) => (
            Some(proposal_id),
            Some(selection_id),
            Some(selection_status),
        ),
        None => (route_proposal_id, None, None),
    };

    Ok(CanonicalStateReread {
        install_url: install_url.to_owned(),
        state_db_path: state_db_path.display().to_string(),
        install_id: Some(install_id),
        proposal_id,
        selection_id,
        onboarding_status: Some(onboarding_status),
        readiness_status: Some(readiness_status),
        selection_status,
    })
}

pub fn highest_observed_phase(run_state: &HarnessRunState) -> HarnessPhase {
    if run_state.selection_id.is_some() {
        HarnessPhase::ApprovalPending
    } else if run_state.proposal_id.is_some() {
        HarnessPhase::RouteReadinessPending
    } else if run_state.install_id.is_some() {
        HarnessPhase::InstallBootstrapPending
    } else {
        run_state.last_phase
    }
}

fn now_unix_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}
