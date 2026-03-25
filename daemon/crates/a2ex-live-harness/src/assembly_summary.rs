use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    evidence_bundle::CanonicalRereadRefs,
    run_state::{HarnessRunState, Verdict},
    verdict::VerdictMismatchDiagnostic,
};

pub const ASSEMBLY_SUMMARY_FILE_NAME: &str = "assembly-summary.json";
pub const ASSEMBLY_REPORT_COMMAND_REF: &str = "scripts/report-m008-s04.sh";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssemblyPhase {
    AwaitingDecisiveSnapshot,
    DecisiveSnapshotFrozen,
    PostControlVerificationPending,
    PostControlVerificationComplete,
}

impl Default for AssemblyPhase {
    fn default() -> Self {
        Self::AwaitingDecisiveSnapshot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssemblyVerificationStatus {
    NotStarted,
    Pass,
    Fail,
    Hold,
    Blocked,
}

impl Default for AssemblyVerificationStatus {
    fn default() -> Self {
        Self::NotStarted
    }
}

impl AssemblyVerificationStatus {
    pub fn from_verdict(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Pass => Self::Pass,
            Verdict::Fail => Self::Fail,
            Verdict::Hold => Self::Hold,
            Verdict::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssemblySummaryRefs {
    pub summary_ref: String,
    pub report_command_ref: String,
}

impl Default for AssemblySummaryRefs {
    fn default() -> Self {
        Self {
            summary_ref: ASSEMBLY_SUMMARY_FILE_NAME.to_owned(),
            report_command_ref: ASSEMBLY_REPORT_COMMAND_REF.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisiveSnapshot {
    pub run_id: String,
    pub captured_at: Option<String>,
    pub verdict: Verdict,
    pub reason_code: String,
    pub summary: String,
    pub decisive_evidence_ref: Option<String>,
    pub decisive_evidence_refs: Vec<String>,
    pub reasoning_evidence_refs: Vec<String>,
    pub reread_snapshot_refs: Vec<String>,
    pub evidence_bundle_ref: String,
    pub evidence_summary_ref: String,
    pub openclaw_action_summary_ref: String,
    pub live_route_evidence_ref: Option<String>,
    pub waiaas_authority_ref: Option<String>,
}

impl DecisiveSnapshot {
    pub fn pending() -> Self {
        Self {
            run_id: String::new(),
            captured_at: None,
            verdict: Verdict::Hold,
            reason_code: "awaiting_live_route_execution".to_owned(),
            summary: "decisive snapshot has not been frozen yet".to_owned(),
            decisive_evidence_ref: Some("live-route-evidence.json".to_owned()),
            decisive_evidence_refs: vec!["live-route-evidence.json".to_owned()],
            reasoning_evidence_refs: vec!["live-route-evidence.json".to_owned()],
            reread_snapshot_refs: CanonicalRereadRefs::default().all_refs(),
            evidence_bundle_ref: "evidence-bundle.json".to_owned(),
            evidence_summary_ref: "evidence-summary.md".to_owned(),
            openclaw_action_summary_ref: "openclaw-action-summary.json".to_owned(),
            live_route_evidence_ref: Some("live-route-evidence.json".to_owned()),
            waiaas_authority_ref: Some("waiaas-authority.json".to_owned()),
        }
    }

    pub fn from_run_state(run_state: &HarnessRunState, captured_at: String) -> Self {
        Self {
            run_id: run_state.run_id.clone(),
            captured_at: Some(captured_at),
            verdict: run_state.final_classification.verdict,
            reason_code: run_state.final_classification.reason_code.clone(),
            summary: run_state.final_classification.summary.clone(),
            decisive_evidence_ref: run_state.final_classification.decisive_evidence_ref.clone(),
            decisive_evidence_refs: run_state
                .final_classification
                .decisive_evidence_refs
                .clone(),
            reasoning_evidence_refs: run_state
                .final_classification
                .reasoning_evidence_refs
                .clone(),
            reread_snapshot_refs: run_state.final_classification.reread_snapshot_refs.clone(),
            evidence_bundle_ref: run_state.evidence_bundle.bundle_ref.clone(),
            evidence_summary_ref: run_state.evidence_bundle.summary_ref.clone(),
            openclaw_action_summary_ref: run_state
                .evidence_bundle
                .openclaw_action_summary_ref
                .clone(),
            live_route_evidence_ref: run_state
                .evidence_refs
                .live_route_evidence
                .evidence_ref
                .clone(),
            waiaas_authority_ref: run_state
                .evidence_refs
                .waiaas_authority
                .evidence_ref
                .clone(),
        }
    }
}

impl Default for DecisiveSnapshot {
    fn default() -> Self {
        Self::pending()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssemblyFailureDiagnostic {
    pub code: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostControlVerification {
    pub report_status: AssemblyVerificationStatus,
    pub runtime_stop_status: AssemblyVerificationStatus,
    pub runtime_clear_stop_status: AssemblyVerificationStatus,
    pub mismatch_status: AssemblyVerificationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_stop_checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_clear_stop_checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mismatch_checked_at: Option<String>,
    // Keep this array present even when empty so run-state.json can prove where
    // snapshot_vs_post_control_mismatch diagnostics would persist later.
    #[serde(default)]
    pub last_lifecycle_check_diagnostics: Vec<AssemblyFailureDiagnostic>,
    pub must_not_overwrite_decisive_verdict: bool,
    pub same_run_id: String,
}

impl Default for PostControlVerification {
    fn default() -> Self {
        Self {
            report_status: AssemblyVerificationStatus::NotStarted,
            runtime_stop_status: AssemblyVerificationStatus::NotStarted,
            runtime_clear_stop_status: AssemblyVerificationStatus::NotStarted,
            mismatch_status: AssemblyVerificationStatus::NotStarted,
            report_checked_at: None,
            runtime_stop_checked_at: None,
            runtime_clear_stop_checked_at: None,
            mismatch_checked_at: None,
            last_lifecycle_check_diagnostics: Vec::new(),
            must_not_overwrite_decisive_verdict: true,
            same_run_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssemblySummary {
    pub run_id: String,
    pub assembly_summary_ref: String,
    pub assembly_phase: AssemblyPhase,
    pub decisive_snapshot: DecisiveSnapshot,
    pub post_control_verification: PostControlVerification,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisive_mismatch_diagnostics: Vec<VerdictMismatchDiagnostic>,
    pub decisive_verdict: Verdict,
    pub decisive_reason_code: String,
    pub decisive_evidence_refs: Vec<String>,
    pub decisive_bundle_ref: String,
    pub decisive_summary_ref: String,
    pub decisive_openclaw_action_summary_ref: String,
    pub post_report_verification: AssemblyVerificationStatus,
    pub post_runtime_stop_verification: AssemblyVerificationStatus,
    pub post_runtime_clear_stop_verification: AssemblyVerificationStatus,
}

impl AssemblySummary {
    pub fn from_run_state(run_state: &HarnessRunState) -> Self {
        Self {
            run_id: run_state.run_id.clone(),
            assembly_summary_ref: run_state.assembly_summary.summary_ref.clone(),
            assembly_phase: run_state.assembly_phase,
            decisive_snapshot: run_state.decisive_snapshot.clone(),
            post_control_verification: run_state.post_control_verification.clone(),
            decisive_mismatch_diagnostics: run_state
                .final_classification
                .mismatch_diagnostics
                .clone(),
            decisive_verdict: run_state.decisive_snapshot.verdict,
            decisive_reason_code: run_state.decisive_snapshot.reason_code.clone(),
            decisive_evidence_refs: run_state.decisive_snapshot.decisive_evidence_refs.clone(),
            decisive_bundle_ref: run_state.decisive_snapshot.evidence_bundle_ref.clone(),
            decisive_summary_ref: run_state.decisive_snapshot.evidence_summary_ref.clone(),
            decisive_openclaw_action_summary_ref: run_state
                .decisive_snapshot
                .openclaw_action_summary_ref
                .clone(),
            post_report_verification: run_state.post_control_verification.report_status,
            post_runtime_stop_verification: run_state.post_control_verification.runtime_stop_status,
            post_runtime_clear_stop_verification: run_state
                .post_control_verification
                .runtime_clear_stop_status,
        }
    }

    pub fn persist_json(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(self).expect("assembly-summary.json serializes"),
        )
    }
}
