use std::path::PathBuf;

use a2ex_skill_bundle::{
    BundleDiagnostic, BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity,
    BundleDocumentLifecycleChange, BundleLifecycleClassification, BundleLifecycleDiagnostic,
    InterpretationBlocker, InterpretationEvidence, SkillBundleInterpretationStatus,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const PROPOSAL_HANDOFF_TOOL_NAME: &str = "skills.load_bundle";
pub const PROPOSAL_HANDOFF_NEXT_TOOL_NAME: &str = "skills.generate_proposal_packet";
pub const PROPOSAL_HANDOFF_PROMPT_NAME: &str = "skills.proposal_packet";
pub const PROPOSAL_HANDOFF_PROPOSAL_RESOURCE_TEMPLATE: &str =
    "a2ex://skills/sessions/{session_id}/proposal";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimDisposition {
    Claimed,
    Reopened,
}

impl ClaimDisposition {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Reopened => "reopened",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "claimed" => Some(Self::Claimed),
            "reopened" => Some(Self::Reopened),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapSource {
    LocalRuntime,
}

impl BootstrapSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LocalRuntime => "local_runtime",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "local_runtime" => Some(Self::LocalRuntime),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallBootstrapRequest {
    pub install_url: Url,
    pub workspace_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_install_id: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapFailureSummary {
    pub stage: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapAttemptMetadata {
    pub attempt_count: u32,
    pub last_attempt_at: Option<String>,
    pub last_completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<BootstrapFailureSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReadiness {
    pub status: SkillBundleInterpretationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<InterpretationBlocker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<BundleDiagnostic>,
}

impl InstallReadiness {
    pub fn blocked_placeholder() -> Self {
        Self {
            status: SkillBundleInterpretationStatus::Blocked,
            blockers: Vec::new(),
            diagnostics: vec![BundleDiagnostic {
                code: BundleDiagnosticCode::LoadNotImplemented,
                severity: BundleDiagnosticSeverity::Error,
                phase: BundleDiagnosticPhase::LoadManifest,
                message: "bundle attachment is not implemented yet".to_owned(),
                document_id: None,
                source_url: None,
                section_slug: None,
            }],
        }
    }

    pub fn status_as_str(&self) -> &'static str {
        match self.status {
            SkillBundleInterpretationStatus::InterpretedReady => "interpreted_ready",
            SkillBundleInterpretationStatus::NeedsSetup => "needs_setup",
            SkillBundleInterpretationStatus::NeedsOwnerDecision => "needs_owner_decision",
            SkillBundleInterpretationStatus::Ambiguous => "ambiguous",
            SkillBundleInterpretationStatus::Blocked => "blocked",
        }
    }

    pub fn status_from_str(value: &str) -> Option<SkillBundleInterpretationStatus> {
        match value {
            "interpreted_ready" => Some(SkillBundleInterpretationStatus::InterpretedReady),
            "needs_setup" => Some(SkillBundleInterpretationStatus::NeedsSetup),
            "needs_owner_decision" => Some(SkillBundleInterpretationStatus::NeedsOwnerDecision),
            "ambiguous" => Some(SkillBundleInterpretationStatus::Ambiguous),
            "blocked" => Some(SkillBundleInterpretationStatus::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingAggregateStatus {
    Ready,
    NeedsAction,
    Blocked,
    Drifted,
}

impl OnboardingAggregateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsAction => "needs_action",
            Self::Blocked => "blocked",
            Self::Drifted => "drifted",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "ready" => Some(Self::Ready),
            "needs_action" => Some(Self::NeedsAction),
            "blocked" => Some(Self::Blocked),
            "drifted" => Some(Self::Drifted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingChecklistSourceKind {
    SetupRequirement,
    OwnerDecision,
    ReadinessBlocker,
    BundleDrift,
}

impl OnboardingChecklistSourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SetupRequirement => "setup_requirement",
            Self::OwnerDecision => "owner_decision",
            Self::ReadinessBlocker => "readiness_blocker",
            Self::BundleDrift => "bundle_drift",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "setup_requirement" => Some(Self::SetupRequirement),
            "owner_decision" => Some(Self::OwnerDecision),
            "readiness_blocker" => Some(Self::ReadinessBlocker),
            "bundle_drift" => Some(Self::BundleDrift),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingChecklistItemStatus {
    Pending,
    Completed,
    Blocked,
    Drifted,
}

impl OnboardingChecklistItemStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Drifted => "drifted",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            "blocked" => Some(Self::Blocked),
            "drifted" => Some(Self::Drifted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingEvidenceKind {
    RequirementIdentity,
    DecisionIdentity,
    DocumentReference,
    DiagnosticReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingEvidenceRef {
    pub kind: OnboardingEvidenceKind,
    pub document_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_slug: Option<String>,
    pub source_url: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingChecklistItem {
    pub checklist_key: String,
    pub source_kind: OnboardingChecklistSourceKind,
    pub status: OnboardingChecklistItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<OnboardingEvidenceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingBundleDrift {
    pub classification: BundleLifecycleClassification,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_documents: Vec<BundleDocumentLifecycleChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<BundleLifecycleDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingView {
    pub aggregate_status: OnboardingAggregateStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checklist_items: Vec<OnboardingChecklistItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<OnboardingBundleDrift>,
}

impl OnboardingView {
    pub fn placeholder_from_readiness(readiness: &InstallReadiness) -> Self {
        let aggregate_status = match readiness.status {
            SkillBundleInterpretationStatus::InterpretedReady => OnboardingAggregateStatus::Ready,
            SkillBundleInterpretationStatus::Blocked => OnboardingAggregateStatus::Blocked,
            SkillBundleInterpretationStatus::NeedsSetup
            | SkillBundleInterpretationStatus::NeedsOwnerDecision
            | SkillBundleInterpretationStatus::Ambiguous => OnboardingAggregateStatus::NeedsAction,
        };

        Self {
            aggregate_status,
            checklist_items: Vec::new(),
            drift: None,
        }
    }

    pub fn from_items(
        aggregate_status: OnboardingAggregateStatus,
        checklist_items: Vec<OnboardingChecklistItem>,
        drift: Option<OnboardingBundleDrift>,
    ) -> Self {
        Self {
            aggregate_status,
            checklist_items,
            drift,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidedOnboardingActionKind {
    Refresh,
    CompleteStep,
    ResolveOwnerDecision,
    AcknowledgeBundleDrift,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingActionRef {
    pub kind: GuidedOnboardingActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingStep {
    pub step_key: String,
    pub status: OnboardingChecklistItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalHandoff {
    pub tool_name: String,
    pub entry_url: Url,
    pub next_tool_name: String,
    pub prompt_name: String,
    pub proposal_resource_template: String,
}

impl ProposalHandoff {
    pub fn from_attached_bundle_url(attached_bundle_url: Url) -> Self {
        Self {
            tool_name: PROPOSAL_HANDOFF_TOOL_NAME.to_owned(),
            entry_url: attached_bundle_url,
            next_tool_name: PROPOSAL_HANDOFF_NEXT_TOOL_NAME.to_owned(),
            prompt_name: PROPOSAL_HANDOFF_PROMPT_NAME.to_owned(),
            proposal_resource_template: PROPOSAL_HANDOFF_PROPOSAL_RESOURCE_TEMPLATE.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessIdentity {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReadinessStatus {
    Ready,
    Incomplete,
    Blocked,
}

impl RouteReadinessStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Incomplete => "incomplete",
            Self::Blocked => "blocked",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "ready" => Some(Self::Ready),
            "incomplete" => Some(Self::Incomplete),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteCapitalReadiness {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_capital_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_capital_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_capital_usd: Option<u64>,
    pub completeness: a2ex_skill_bundle::ProposalQuantitativeCompleteness,
    pub summary: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteApprovalTuple {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessBlocker {
    pub code: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteOwnerAction {
    pub kind: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReadinessActionKind {
    Refresh,
    CompleteStep,
    ReviewStaleReadiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReadinessStepStatus {
    Pending,
    Completed,
    Stale,
}

impl RouteReadinessStepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Stale => "stale",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessActionRef {
    pub kind: RouteReadinessActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessStep {
    pub step_key: String,
    pub status: RouteReadinessStepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<RouteReadinessActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReadinessStaleStatus {
    Fresh,
    Stale,
}

impl RouteReadinessStaleStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "fresh" => Some(Self::Fresh),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessStaleState {
    pub status: RouteReadinessStaleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessActionRejection {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessEvaluationMetadata {
    pub request_id: String,
    pub evaluated_at: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessRecord {
    pub identity: RouteReadinessIdentity,
    pub status: RouteReadinessStatus,
    pub capital: RouteCapitalReadiness,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<RouteApprovalTuple>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<RouteReadinessBlocker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_owner_action: Option<RouteOwnerAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordered_steps: Vec<RouteReadinessStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<RouteReadinessActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<RouteReadinessStaleState>,
    #[serde(default)]
    pub last_rejection: Option<RouteReadinessActionRejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<RouteReadinessEvaluationMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessEvaluationRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessInspectionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteReadinessAction {
    Refresh,
    CompleteStep { step_key: String },
    ReviewStaleReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessActionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub action: RouteReadinessAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessActionResult {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub status: RouteReadinessStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<RouteReadinessActionRef>,
    pub stale: RouteReadinessStaleState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingStateRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingState {
    pub install_id: String,
    pub workspace_id: String,
    pub attached_bundle_url: Url,
    pub aggregate_status: OnboardingAggregateStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordered_steps: Vec<GuidedOnboardingStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_handoff: Option<ProposalHandoff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<OnboardingBundleDrift>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuidedOnboardingAction {
    Refresh,
    CompleteStep {
        step_key: String,
    },
    ResolveOwnerDecision {
        step_key: String,
        resolution: String,
    },
    AcknowledgeBundleDrift,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingActionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub action: GuidedOnboardingAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingActionResult {
    pub install_id: String,
    pub aggregate_status: OnboardingAggregateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingActionRejection {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingInspectionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidedOnboardingInspection {
    pub install_id: String,
    pub workspace_id: String,
    pub attached_bundle_url: Url,
    pub bootstrap: BootstrapReport,
    pub aggregate_status: OnboardingAggregateStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordered_steps: Vec<GuidedOnboardingStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_handoff: Option<ProposalHandoff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checklist_items: Vec<OnboardingChecklistItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<OnboardingBundleDrift>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection: Option<GuidedOnboardingActionRejection>,
}

impl From<GuidedOnboardingInspection> for GuidedOnboardingState {
    fn from(value: GuidedOnboardingInspection) -> Self {
        Self {
            install_id: value.install_id,
            workspace_id: value.workspace_id,
            attached_bundle_url: value.attached_bundle_url,
            aggregate_status: value.aggregate_status,
            ordered_steps: value.ordered_steps,
            current_step_key: value.current_step_key,
            recommended_action: value.recommended_action,
            proposal_handoff: value.proposal_handoff,
            drift: value.drift,
        }
    }
}

impl GuidedOnboardingState {
    pub fn into_action_result(self) -> GuidedOnboardingActionResult {
        GuidedOnboardingActionResult {
            install_id: self.install_id,
            aggregate_status: self.aggregate_status,
            current_step_key: self.current_step_key,
            recommended_action: self.recommended_action,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallBootstrapResult {
    pub workspace_id: String,
    pub install_id: String,
    pub claim_disposition: ClaimDisposition,
    pub claimed_workspace_root: PathBuf,
    pub attached_bundle_url: Url,
    pub bootstrap: BootstrapReport,
    pub readiness: InstallReadiness,
    pub onboarding: OnboardingView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_handoff: Option<ProposalHandoff>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySelectionStatus {
    Recommended,
    Approved,
    Reopened,
}

impl StrategySelectionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Recommended => "recommended",
            Self::Approved => "approved",
            Self::Reopened => "reopened",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "recommended" => Some(Self::Recommended),
            "approved" => Some(Self::Approved),
            "reopened" => Some(Self::Reopened),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySelectionSensitivityClass {
    Advisory,
    ReadinessSensitive,
}

impl StrategySelectionSensitivityClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Advisory => "advisory",
            Self::ReadinessSensitive => "readiness_sensitive",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "advisory" => Some(Self::Advisory),
            "readiness_sensitive" => Some(Self::ReadinessSensitive),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionRecommendationBasis {
    pub source_kind: String,
    pub proposal_uri: String,
    pub proposal_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionReadinessSensitivitySummary {
    #[serde(default)]
    pub readiness_sensitive_override_keys: Vec<String>,
    #[serde(default)]
    pub advisory_override_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionApprovalState {
    pub status: String,
    pub approved_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionRecord {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: StrategySelectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopened_from_revision: Option<u32>,
    pub proposal_revision: u64,
    pub proposal_uri: String,
    pub proposal_snapshot: serde_json::Value,
    pub recommendation_basis: StrategySelectionRecommendationBasis,
    pub readiness_sensitivity_summary: StrategySelectionReadinessSensitivitySummary,
    pub approval: StrategySelectionApprovalState,
    pub approval_stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_stale_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionApprovalHistoryEvent {
    pub event_kind: String,
    pub selection_revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopened_from_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<serde_json::Value>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionEffectiveDiff {
    pub baseline_kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_override_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_sensitive_changes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisory_changes: Vec<String>,
    pub readiness_stale: bool,
    pub approval_stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_stale_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionDiscussion {
    pub recommendation_basis: StrategySelectionRecommendationBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionOverrideRecord {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub override_key: String,
    pub previous_value: serde_json::Value,
    pub new_value: serde_json::Value,
    pub rationale: String,
    pub provenance: serde_json::Value,
    pub sensitivity_class: StrategySelectionSensitivityClass,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializeStrategySelectionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub proposal_uri: String,
    pub proposal_revision: u64,
    pub proposal: a2ex_skill_bundle::SkillProposalPacket,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyStrategySelectionOverride {
    pub key: String,
    pub value: serde_json::Value,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyStrategySelectionOverrideRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub override_record: ApplyStrategySelectionOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionApprovalInput {
    pub approved_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApproveStrategySelectionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub expected_selection_revision: u32,
    pub approval: StrategySelectionApprovalInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReopenStrategySelectionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectStrategySelectionRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionInspection {
    pub summary: StrategySelectionRecord,
    #[serde(default)]
    pub overrides: Vec<StrategySelectionOverrideRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_history: Vec<StrategySelectionApprovalHistoryEvent>,
    pub effective_diff: StrategySelectionEffectiveDiff,
    pub discussion: StrategySelectionDiscussion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyRuntimeEligibilityStatus {
    Eligible,
    Blocked,
}

impl StrategyRuntimeEligibilityStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::Blocked => "blocked",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "eligible" => Some(Self::Eligible),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyRuntimeHoldReason {
    RouteReadinessStale,
    RouteReadinessIncomplete,
    RouteReadinessMissing,
    ApprovedSelectionRevisionStale,
    RuntimeControlPaused,
    RuntimeControlStopped,
}

impl StrategyRuntimeHoldReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RouteReadinessStale => "route_readiness_stale",
            Self::RouteReadinessIncomplete => "route_readiness_incomplete",
            Self::RouteReadinessMissing => "route_readiness_missing",
            Self::ApprovedSelectionRevisionStale => "approved_selection_revision_stale",
            Self::RuntimeControlPaused => "runtime_control_paused",
            Self::RuntimeControlStopped => "runtime_control_stopped",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "route_readiness_stale" => Some(Self::RouteReadinessStale),
            "route_readiness_incomplete" => Some(Self::RouteReadinessIncomplete),
            "route_readiness_missing" => Some(Self::RouteReadinessMissing),
            "approved_selection_revision_stale" => Some(Self::ApprovedSelectionRevisionStale),
            "runtime_control_paused" => Some(Self::RuntimeControlPaused),
            "runtime_control_stopped" => Some(Self::RuntimeControlStopped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeHandoffRecord {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub approved_selection_revision: u32,
    pub route_id: String,
    pub request_id: String,
    pub route_readiness_fingerprint: String,
    pub route_readiness_status: RouteReadinessStatus,
    pub route_readiness_evaluated_at: String,
    pub eligibility_status: StrategyRuntimeEligibilityStatus,
    #[serde(default)]
    pub hold_reason: Option<StrategyRuntimeHoldReason>,
    pub runtime_control_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_identity_refreshed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_identity_source: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectStrategyRuntimeRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyRuntimePhase {
    AwaitingRuntimeIdentity,
    Idle,
    Active,
    Rebalancing,
    SyncingHedge,
    Recovering,
    Unwinding,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyRuntimeControlGateStatus {
    Open,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeActionView {
    pub kind: String,
    pub status: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeLastOutcome {
    pub code: String,
    pub message: String,
    pub observed_at: String,
}

pub const STRATEGY_OPERATOR_REPORT_KIND: &str = "strategy_operator_report";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyOperatorReportFreshness {
    pub reported_at: String,
    pub route_readiness_evaluated_at: String,
    pub runtime_control_updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_identity_refreshed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation_observed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyOperatorReconciliationStatus {
    NotStarted,
    Pending,
    Reconciled,
    RebalanceRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyOperatorReconciliationEvidence {
    pub status: StrategyOperatorReconciliationStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_exposure_usd: Option<i64>,
    pub rebalance_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyOperatorReport {
    pub report_kind: String,
    pub freshness: StrategyOperatorReportFreshness,
    pub phase: StrategyRuntimePhase,
    #[serde(default)]
    pub last_action: Option<StrategyRuntimeActionView>,
    #[serde(default)]
    pub next_intended_action: Option<StrategyRuntimeActionView>,
    #[serde(default)]
    pub hold_reason: Option<StrategyRuntimeHoldReason>,
    pub control_mode: String,
    pub reconciliation_evidence: StrategyOperatorReconciliationEvidence,
    pub owner_action_needed: bool,
    pub recommended_operator_action: String,
    #[serde(default)]
    pub last_runtime_failure: Option<StrategyRuntimeLastOutcome>,
    #[serde(default)]
    pub last_runtime_rejection: Option<StrategyRuntimeLastOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeMonitoringSummary {
    #[serde(flatten)]
    pub handoff: StrategyRuntimeHandoffRecord,
    pub report_kind: String,
    pub freshness: StrategyOperatorReportFreshness,
    pub phase: StrategyRuntimePhase,
    pub current_phase: StrategyRuntimePhase,
    pub runtime_control_gate_status: StrategyRuntimeControlGateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_action: Option<StrategyRuntimeActionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_intended_action: Option<StrategyRuntimeActionView>,
    pub control_mode: String,
    pub runtime_control: RuntimeControlSnapshot,
    pub reconciliation_evidence: StrategyOperatorReconciliationEvidence,
    pub owner_action_needed: bool,
    pub recommended_operator_action: String,
    #[serde(default)]
    pub last_runtime_failure: Option<StrategyRuntimeLastOutcome>,
    #[serde(default)]
    pub last_runtime_rejection: Option<StrategyRuntimeLastOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_operator_guidance: Option<StrategyRuntimeOperatorGuidance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeOperatorGuidance {
    pub recommended_action: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeControlSnapshot {
    pub control_mode: String,
    pub transition_reason: String,
    pub transition_source: String,
    pub transitioned_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection_operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection_at: Option<String>,
    pub updated_at: String,
}

pub const STRATEGY_REPORT_WINDOW_KIND: &str = "strategy_report_window";
pub const STRATEGY_EXCEPTION_ROLLUP_KIND: &str = "strategy_exception_rollup";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectStrategyReportWindowRequest {
    pub state_db_path: PathBuf,
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub cursor: String,
    pub window_limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyReportWindowIdentity {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyReportWindowChangeKind {
    SelectionMaterialized,
    SelectionOverrideApplied,
    SelectionApproved,
    SelectionReopened,
    RuntimeHandoffPersisted,
    RuntimeEligibilityChanged,
    RuntimeIdentityRefreshed,
    RuntimeControlChanged,
    RuntimeStateChanged,
    ExecutionStateChanged,
    ReconciliationStateChanged,
    JournalEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyReportWindowChange {
    pub cursor: String,
    pub change_kind: StrategyReportWindowChangeKind,
    pub observed_at: String,
    pub summary: String,
    pub operator_impact: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyExceptionUrgency {
    Monitor,
    InvestigateSoon,
    ActionRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeHoldException {
    pub reason_code: StrategyRuntimeHoldReason,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyExceptionRollup {
    pub report_kind: String,
    pub identity: StrategyReportWindowIdentity,
    pub owner_action_needed_now: bool,
    pub urgency: StrategyExceptionUrgency,
    pub recommended_operator_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_hold: Option<StrategyRuntimeHoldException>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_runtime_failure: Option<StrategyRuntimeLastOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_runtime_rejection: Option<StrategyRuntimeLastOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyReportWindow {
    pub report_kind: String,
    pub identity: StrategyReportWindowIdentity,
    pub cursor: String,
    pub window_start_cursor: String,
    pub window_end_cursor: String,
    pub window_limit: u32,
    pub freshness: StrategyOperatorReportFreshness,
    pub current_operator_report: StrategyOperatorReport,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_changes: Vec<StrategyReportWindowChange>,
    pub exception_rollup: StrategyExceptionRollup,
    pub owner_action_needed_now: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallBootstrapError {
    #[error("workspace identity mismatch")]
    WorkspaceIdentityMismatch {
        existing_workspace_id: String,
        existing_install_id: String,
    },
    #[error("install identity mismatch")]
    InstallIdentityMismatch {
        existing_workspace_id: String,
        existing_install_id: String,
    },
    #[error("bootstrap persistence failed: {message}")]
    Persistence { message: String },
    #[error("daemon bootstrap failed: {message}")]
    DaemonBootstrap { message: String },
    #[error("bundle load failed for {attached_bundle_url}: {message}")]
    BundleLoad {
        attached_bundle_url: Url,
        message: String,
    },
    #[error("bundle interpretation failed for {attached_bundle_url}: {message}")]
    BundleInterpretation {
        attached_bundle_url: Url,
        message: String,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuidedOnboardingError {
    #[error("guided onboarding is not implemented for {operation}")]
    NotImplemented { operation: &'static str },
    #[error("guided onboarding action rejected: {code}: {message}")]
    ActionRejected { code: String, message: String },
    #[error("guided onboarding persistence failed: {message}")]
    Persistence { message: String },
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteReadinessError {
    #[error("route readiness is not implemented for {operation}")]
    NotImplemented { operation: &'static str },
    #[error("route readiness action rejected: {code}: {message}")]
    ActionRejected { code: String, message: String },
    #[error("route readiness persistence failed: {message}")]
    Persistence { message: String },
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrategySelectionError {
    #[error("strategy selection not found for install {install_id} proposal {proposal_id}")]
    NotFound {
        install_id: String,
        proposal_id: String,
    },
    #[error("strategy selection rejected: invalid_override_key: {override_key}")]
    InvalidOverrideKey { override_key: String },
    #[error(
        "strategy selection rejected: stale_selection_revision expected {expected_selection_revision} but found {actual_selection_revision}"
    )]
    StaleSelectionRevision {
        expected_selection_revision: u32,
        actual_selection_revision: u32,
    },
    #[error("strategy selection persistence failed: {message}")]
    Persistence { message: String },
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrategyRuntimeHandoffError {
    #[error(
        "strategy runtime handoff not found for install {install_id} proposal {proposal_id} selection {selection_id}"
    )]
    NotFound {
        install_id: String,
        proposal_id: String,
        selection_id: String,
    },
    #[error("strategy runtime handoff persistence failed: {message}")]
    Persistence { message: String },
}
