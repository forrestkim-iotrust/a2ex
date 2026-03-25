mod interpret;
mod lifecycle;
mod loader;
mod model;
mod parser;
mod proposal;

pub use interpret::{interpret_bundle_load_outcome, interpret_skill_bundle};
pub use loader::load_skill_bundle_from_url;
pub use model::{
    BundleDiagnostic, BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity,
    BundleDocument, BundleDocumentLifecycleChange, BundleDocumentLifecycleChangeKind,
    BundleDocumentManifestEntry, BundleDocumentRole, BundleLifecycleChange,
    BundleLifecycleClassification, BundleLifecycleDaemonCompatibility, BundleLifecycleDiagnostic,
    BundleLifecycleDiagnosticCode, BundleLifecycleDocumentSnapshot,
    BundleLifecycleManifestEntrySnapshot, BundleLifecycleSnapshot, BundleLoadOutcome,
    BundleSection, BundleSectionKind, FetchedBundleDocument, InterpretationAmbiguity,
    InterpretationAutomationBoundary, InterpretationBlocker, InterpretationEvidence,
    InterpretationOwnerDecision, InterpretationPlanSummary, InterpretationRisk,
    InterpretationSetupRequirement, InterpretationSetupRequirementKind, ParsedSkillBundle,
    ProposalQuantitativeCompleteness, ProposalQuantitativeProfile, ProposalReadiness, SkillBundle,
    SkillBundleInterpretation, SkillBundleInterpretationStatus, SkillProposalPacket,
    UnresolvedBundleSection,
};
pub use parser::parse_skill_bundle_documents;
pub use proposal::generate_proposal_packet;

use reqwest::Url;
use thiserror::Error;

pub type BundleResult<T> = Result<T, BundleError>;

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("bundle operation emitted diagnostics")]
    Diagnostics { diagnostics: Vec<BundleDiagnostic> },
    #[error("bundle transport failed for {source_url}: {source}")]
    Transport {
        source_url: Url,
        #[source]
        source: reqwest::Error,
    },
    #[error("bundle load is not implemented yet: {message}")]
    NotImplemented { message: String },
}
