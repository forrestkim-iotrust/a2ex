use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundle {
    pub bundle_id: String,
    pub bundle_format: String,
    pub bundle_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatible_daemon: Option<String>,
    pub entry_document_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub document_manifest: Vec<BundleDocumentManifestEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<BundleDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_sections: Vec<UnresolvedBundleSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleDocumentManifestEntry {
    pub document_id: String,
    pub role: BundleDocumentRole,
    pub relative_path: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleDocument {
    pub document_id: String,
    pub role: BundleDocumentRole,
    pub source_url: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub body_markdown: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<BundleSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleSection {
    pub section_id: String,
    pub document_id: String,
    pub section_heading: String,
    pub section_slug: String,
    pub heading_level: u8,
    pub kind: BundleSectionKind,
    pub source_url: Url,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedBundleSection {
    pub section_id: String,
    pub document_id: String,
    pub section_heading: String,
    pub section_slug: String,
    pub heading_level: u8,
    pub source_url: Url,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchedBundleDocument {
    pub document_id: String,
    pub source_url: Url,
    pub body_markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedSkillBundle {
    pub bundle: SkillBundle,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<BundleDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLoadOutcome {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<SkillBundle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<BundleDiagnostic>,
}

impl BundleLoadOutcome {
    pub fn lifecycle_change_from(
        &self,
        previous: Option<&BundleLoadOutcome>,
    ) -> BundleLifecycleChange {
        crate::lifecycle::diff_bundle_lifecycle(self, previous)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLifecycleChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<BundleLifecycleSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<BundleLifecycleSnapshot>,
    pub classification: BundleLifecycleClassification,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_documents: Vec<BundleDocumentLifecycleChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<BundleLifecycleDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLifecycleSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatible_daemon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_compatibility: Option<BundleLifecycleDaemonCompatibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manifest_entries: Vec<BundleLifecycleManifestEntrySnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<BundleLifecycleDocumentSnapshot>,
}

impl BundleLifecycleSnapshot {
    pub fn documents_by_id(
        &self,
    ) -> std::collections::BTreeMap<&str, &BundleLifecycleDocumentSnapshot> {
        self.documents
            .iter()
            .map(|document| (document.document_id.as_str(), document))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLifecycleDaemonCompatibility {
    pub daemon_version: String,
    pub requirement: String,
    pub is_compatible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLifecycleManifestEntrySnapshot {
    pub document_id: String,
    pub role: BundleDocumentRole,
    pub relative_path: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_source_url: Option<Url>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLifecycleDocumentSnapshot {
    pub document_id: String,
    pub role: BundleDocumentRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleDocumentLifecycleChange {
    pub document_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<BundleDocumentRole>,
    pub kind: BundleDocumentLifecycleChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<Url>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLifecycleDiagnostic {
    pub code: BundleLifecycleDiagnosticCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<Url>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BundleLifecycleClassification {
    #[default]
    NoChange,
    MetadataChanged,
    DocumentsChanged,
    BlockingDrift,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleDocumentLifecycleChangeKind {
    Added,
    Removed,
    RevisionChanged,
    ContentChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleLifecycleDiagnosticCode {
    IncompatibleDaemon,
    ManifestDocumentRevisionMismatch,
    MissingDocument,
    RemovedDocument,
    UnreadableDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleDiagnostic {
    pub code: BundleDiagnosticCode,
    pub severity: BundleDiagnosticSeverity,
    pub phase: BundleDiagnosticPhase,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_slug: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleDocumentRole {
    Entry,
    OwnerSetup,
    OperatorNotes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleSectionKind {
    Overview,
    OwnerDecisions,
    RequiredSecrets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleDiagnosticCode {
    MalformedFrontmatter,
    MissingRequiredMetadata,
    MissingRequiredDocument,
    DuplicateDocumentId,
    DuplicateDocumentRole,
    FetchFailed,
    InvalidDocumentReference,
    ReferenceCycle,
    ReferenceDepthExceeded,
    LoadNotImplemented,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleDiagnosticPhase {
    ParseDocument,
    LoadManifest,
    ResolveDocument,
    FetchDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillBundleInterpretationStatus {
    InterpretedReady,
    NeedsSetup,
    NeedsOwnerDecision,
    Ambiguous,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundleInterpretation {
    pub status: SkillBundleInterpretationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_summary: Option<InterpretationPlanSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owner_decisions: Vec<InterpretationOwnerDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup_requirements: Vec<InterpretationSetupRequirement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation_boundaries: Vec<InterpretationAutomationBoundary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risks: Vec<InterpretationRisk>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguities: Vec<InterpretationAmbiguity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<InterpretationBlocker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProposalPacket {
    pub interpretation_status: SkillBundleInterpretationStatus,
    pub proposal_readiness: ProposalReadiness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_summary: Option<InterpretationPlanSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup_requirements: Vec<InterpretationSetupRequirement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owner_override_points: Vec<InterpretationOwnerDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation_boundaries: Vec<InterpretationAutomationBoundary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_framing: Vec<InterpretationRisk>,
    pub capital_profile: ProposalQuantitativeProfile,
    pub cost_profile: ProposalQuantitativeProfile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguities: Vec<InterpretationAmbiguity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<InterpretationBlocker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalReadiness {
    Ready,
    Incomplete,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalQuantitativeProfile {
    pub completeness: ProposalQuantitativeCompleteness,
    pub summary: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalQuantitativeCompleteness {
    Complete,
    Unknown,
    RequiresOwnerInput,
    NotInBundleContract,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationPlanSummary {
    pub bundle_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationOwnerDecision {
    pub decision_key: String,
    pub decision_text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationSetupRequirement {
    pub requirement_key: String,
    pub requirement_kind: InterpretationSetupRequirementKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterpretationSetupRequirementKind {
    Secret,
    Environment,
    Approval,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationAutomationBoundary {
    pub boundary_key: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationRisk {
    pub risk_key: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationAmbiguity {
    pub ambiguity_key: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationBlocker {
    pub blocker_key: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<BundleDiagnosticCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_severity: Option<BundleDiagnosticSeverity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_phase: Option<BundleDiagnosticPhase>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub struct InterpretationEvidence {
    pub document_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_slug: Option<String>,
    pub source_url: Url,
}
