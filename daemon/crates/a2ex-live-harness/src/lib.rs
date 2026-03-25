pub mod a2ex_rereads;
pub mod assembly_summary;
pub mod error;
pub mod evidence_bundle;
pub mod live_route;
pub mod openclaw;
pub mod preflight;
pub mod run_state;
pub mod runtime_metadata;
pub mod verdict;
pub mod waiaas;

pub use a2ex_rereads::{
    A2exCanonicalRereads, A2exRereadDiagnostic, RuntimeControlFailuresSnapshot,
    RuntimeControlRejectionSnapshot, RuntimeControlStatusSnapshot, collect_canonical_rereads,
};
pub use assembly_summary::{
    ASSEMBLY_REPORT_COMMAND_REF, ASSEMBLY_SUMMARY_FILE_NAME, AssemblyFailureDiagnostic,
    AssemblyPhase, AssemblySummary, AssemblySummaryRefs, AssemblyVerificationStatus,
    DecisiveSnapshot, PostControlVerification,
};
pub use error::{ErrorClass, FailureKind, HarnessError, HarnessIssue, HarnessResult};
pub use evidence_bundle::{
    CanonicalRereadRef, CanonicalRereadRefs, EVIDENCE_BUNDLE_FILE_NAME, EVIDENCE_SUMMARY_FILE_NAME,
    EvidenceBundle, EvidenceBundleRefs, EvidenceBundleSection, OPENCLAW_ACTION_SUMMARY_FILE_NAME,
    OpenClawActionPhase, OpenClawActionSummary, render_evidence_summary_markdown,
};
pub use live_route::{
    LiveRiskEnvelope, LiveRouteContract, LiveSuccessCriteria, RequiredEvidenceField, S02_ROUTE_ID,
    S02_ROUTE_RISK_ENVELOPE, S02_ROUTE_SUCCESS_SIGNAL, s02_live_route_contract,
};
pub use openclaw::{
    OpenClawExit, OpenClawLaunchArtifacts, OpenClawLaunchHandle, OpenClawLaunchMode,
    OpenClawLaunchRequest, default_guidance_contract, persist_action_summary,
};
pub use preflight::{
    LocalPreflightProbe, PreflightCheckResult, PreflightConfig, PreflightProbe, PreflightReport,
    PreflightStatus, ProbeOutcome, ProbeSnapshot, WaiaasAuthorityClassification,
    WaiaasAuthoritySemantics, build_report, run_preflight,
};
pub use run_state::{
    AuthorityDecision, CanonicalCorrelation, CanonicalStateReread, EvidenceRef, EvidenceRefs,
    FinalClassification, HarnessPhase, HarnessRunState, InstallLocatorStatus, LaunchMetadata,
    LiveAttemptPhase, LiveAttemptState, LiveAttemptTransition, PhaseTransition, PreTradeOutcome,
    ReconnectStatus, Verdict, WaiaasAuthorityState, WaiaasInspection, highest_observed_phase,
    reread_canonical_state,
};
pub use runtime_metadata::{
    A2exRuntimeMetadata, EnvVarStatus, OpenClawRuntimeMetadata, RuntimeMetadata,
    RuntimeMetadataInput, WaiaasRuntimeMetadata,
};
pub use verdict::{
    ClassifiedVerdict, LiveRouteVerdictState, VerdictClassifierInput, VerdictMismatchDiagnostic,
    classify_verdict,
};
pub use waiaas::{
    DEFAULT_AUTHORITY_EVIDENCE_REF, DEFAULT_WALLET_BOUNDARY, HttpWaiaasAuthorityAdapter,
    WaiaasAuthorityAdapter, WaiaasAuthorityCapture, WaiaasAuthorityEvidence,
    WaiaasAuthorityOutcome, WaiaasAuthorityRequest, WaiaasProbeEvidence,
};
