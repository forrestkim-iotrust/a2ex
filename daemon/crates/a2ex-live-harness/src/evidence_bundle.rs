use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    a2ex_rereads::A2exCanonicalRereads,
    assembly_summary::ASSEMBLY_SUMMARY_FILE_NAME,
    run_state::{FinalClassification, HarnessRunState},
    runtime_metadata::RuntimeMetadata,
};

pub const EVIDENCE_BUNDLE_FILE_NAME: &str = "evidence-bundle.json";
pub const EVIDENCE_SUMMARY_FILE_NAME: &str = "evidence-summary.md";
pub const OPENCLAW_ACTION_SUMMARY_FILE_NAME: &str = "openclaw-action-summary.json";
pub const REPORT_COMMAND_REF: &str = "scripts/report-m008-s03.sh";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceBundleRefs {
    pub bundle_ref: String,
    pub summary_ref: String,
    pub openclaw_action_summary_ref: String,
    pub assembly_summary_ref: String,
    pub report_command_ref: String,
    pub pinned_runtime_metadata: RuntimeMetadata,
}

impl EvidenceBundleRefs {
    pub fn for_runtime_metadata(runtime_metadata: RuntimeMetadata) -> Self {
        Self {
            bundle_ref: EVIDENCE_BUNDLE_FILE_NAME.to_owned(),
            summary_ref: EVIDENCE_SUMMARY_FILE_NAME.to_owned(),
            openclaw_action_summary_ref: OPENCLAW_ACTION_SUMMARY_FILE_NAME.to_owned(),
            assembly_summary_ref: ASSEMBLY_SUMMARY_FILE_NAME.to_owned(),
            report_command_ref: REPORT_COMMAND_REF.to_owned(),
            pinned_runtime_metadata: runtime_metadata,
        }
    }
}

impl Default for EvidenceBundleRefs {
    fn default() -> Self {
        Self::for_runtime_metadata(RuntimeMetadata::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalRereadRef {
    pub label: String,
    pub evidence_ref: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalRereadRefs {
    pub operator_report: CanonicalRereadRef,
    pub report_window: CanonicalRereadRef,
    pub exception_rollup: CanonicalRereadRef,
    pub runtime_control_status: CanonicalRereadRef,
    pub runtime_control_failures: CanonicalRereadRef,
}

impl CanonicalRereadRefs {
    pub fn for_selection(
        install_id: Option<&str>,
        proposal_id: Option<&str>,
        selection_id: Option<&str>,
    ) -> Self {
        let install_id = install_id.unwrap_or("{install_id}");
        let proposal_id = proposal_id.unwrap_or("{proposal_id}");
        let selection_id = selection_id.unwrap_or("{selection_id}");
        let strategy_prefix =
            format!("a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}");
        let runtime_prefix = format!("a2ex://runtime/control/{install_id}");

        Self {
            operator_report: CanonicalRereadRef {
                label: "operator-report".to_owned(),
                evidence_ref: format!("{strategy_prefix}/operator-report"),
                description:
                    "canonical operator-report reread for reconnect-safe terminal evidence"
                        .to_owned(),
            },
            report_window: CanonicalRereadRef {
                label: "report-window".to_owned(),
                evidence_ref: format!("{strategy_prefix}/report-window/bootstrap"),
                description: "canonical report-window reread for bounded recent-change inspection"
                    .to_owned(),
            },
            exception_rollup: CanonicalRereadRef {
                label: "exception-rollup".to_owned(),
                evidence_ref: format!("{strategy_prefix}/exception-rollup"),
                description:
                    "canonical exception-rollup reread for typed hold/failure/rejection inspection"
                        .to_owned(),
            },
            runtime_control_status: CanonicalRereadRef {
                label: "runtime control status".to_owned(),
                evidence_ref: format!("{runtime_prefix}/status"),
                description:
                    "canonical runtime control status reread for install-scoped autonomy state"
                        .to_owned(),
            },
            runtime_control_failures: CanonicalRereadRef {
                label: "runtime control failures".to_owned(),
                evidence_ref: format!("{runtime_prefix}/failures"),
                description:
                    "canonical runtime control failures reread for durable rejection diagnostics"
                        .to_owned(),
            },
        }
    }

    pub fn all_refs(&self) -> Vec<String> {
        vec![
            self.operator_report.evidence_ref.clone(),
            self.report_window.evidence_ref.clone(),
            self.exception_rollup.evidence_ref.clone(),
            self.runtime_control_status.evidence_ref.clone(),
            self.runtime_control_failures.evidence_ref.clone(),
        ]
    }
}

impl Default for CanonicalRereadRefs {
    fn default() -> Self {
        Self::for_selection(None, None, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenClawActionPhase {
    pub phase_key: String,
    pub action_name: String,
    pub status: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenClawActionSummary {
    pub run_id: String,
    pub install_url: Option<String>,
    pub request_source: String,
    pub request_path: String,
    pub guidance_path: String,
    pub stdout_path: String,
    pub stderr_path: String,
    pub launch_mode: String,
    pub runtime_command_ref: String,
    pub image_ref: Option<String>,
    pub overall_status: String,
    pub stop_reason: Option<String>,
    pub stdout_line_count: usize,
    pub stderr_line_count: usize,
    pub typed_phase_summary: Vec<OpenClawActionPhase>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceBundleSection {
    pub section_key: String,
    pub title: String,
    pub summary: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub run_id: String,
    pub install_id: Option<String>,
    pub proposal_id: Option<String>,
    pub selection_id: Option<String>,
    pub evidence_bundle: EvidenceBundleRefs,
    pub canonical_rereads: CanonicalRereadRefs,
    pub canonical_reread_collection: A2exCanonicalRereads,
    pub runtime_metadata: RuntimeMetadata,
    pub openclaw_action_summary: OpenClawActionSummary,
    pub final_classification: FinalClassification,
    pub sections: Vec<EvidenceBundleSection>,
}

impl EvidenceBundle {
    pub fn from_run_state(
        run_state: &HarnessRunState,
        openclaw_action_summary: OpenClawActionSummary,
    ) -> Self {
        let sections = vec![
            EvidenceBundleSection {
                section_key: "runtime_metadata".to_owned(),
                title: "Pinned runtime metadata".to_owned(),
                summary:
                    "Pinned runtime_metadata keeps the OpenClaw, A2EX, and WAIaaS runtime boundary stable for this run_id."
                        .to_owned(),
                evidence_refs: vec![run_state.evidence_bundle.bundle_ref.clone()],
            },
            EvidenceBundleSection {
                section_key: "openclaw_action_summary".to_owned(),
                title: "OpenClaw action summary".to_owned(),
                summary:
                    "Secret-safe OpenClaw action summary derived from persisted request, guidance, and log artifact refs."
                        .to_owned(),
                evidence_refs: vec![run_state.evidence_bundle.openclaw_action_summary_ref.clone()],
            },
            EvidenceBundleSection {
                section_key: "canonical_rereads".to_owned(),
                title: "Canonical rereads".to_owned(),
                summary:
                    "Canonical rereads point to operator-report, report-window, exception-rollup, and runtime control status/failures for reconnect-safe inspection."
                        .to_owned(),
                evidence_refs: run_state.canonical_rereads.all_refs(),
            },
            EvidenceBundleSection {
                section_key: "final_verdict".to_owned(),
                title: "Final verdict".to_owned(),
                summary: run_state.final_classification.reasoning_summary.clone(),
                evidence_refs: run_state.final_classification.decisive_evidence_refs.clone(),
            },
            EvidenceBundleSection {
                section_key: "assembly_summary".to_owned(),
                title: "S04 assembly summary".to_owned(),
                summary:
                    "S04 assembly-summary.json freezes the decisive snapshot and carries later post-control verification under the same run_id without overwriting S03 truth."
                        .to_owned(),
                evidence_refs: vec![run_state.evidence_bundle.assembly_summary_ref.clone()],
            },
        ];

        Self {
            run_id: run_state.run_id.clone(),
            install_id: run_state.install_id.clone(),
            proposal_id: run_state.proposal_id.clone(),
            selection_id: run_state.selection_id.clone(),
            evidence_bundle: run_state.evidence_bundle.clone(),
            canonical_rereads: run_state.canonical_rereads.clone(),
            canonical_reread_collection: run_state.canonical_reread_collection.clone(),
            runtime_metadata: run_state.runtime_metadata.clone(),
            openclaw_action_summary,
            final_classification: run_state.final_classification.clone(),
            sections,
        }
    }

    pub fn persist_json(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(self).expect("evidence bundle serializes"),
        )
    }
}

pub fn render_evidence_summary_markdown(bundle: &EvidenceBundle) -> String {
    let mut markdown = vec![
        "# A2EX S03 evidence summary".to_owned(),
        String::new(),
        format!("- run_id: `{}`", bundle.run_id),
        format!(
            "- install_id: `{}`",
            bundle.install_id.as_deref().unwrap_or("pending")
        ),
        format!(
            "- proposal_id: `{}`",
            bundle.proposal_id.as_deref().unwrap_or("pending")
        ),
        format!(
            "- selection_id: `{}`",
            bundle.selection_id.as_deref().unwrap_or("pending")
        ),
        format!(
            "- verdict: `{}`",
            bundle.final_classification.verdict.as_str()
        ),
        format!(
            "- reason_code: `{}`",
            bundle.final_classification.reason_code
        ),
        format!("- bundle_ref: `{}`", bundle.evidence_bundle.bundle_ref),
        format!("- summary_ref: `{}`", bundle.evidence_bundle.summary_ref),
        format!(
            "- openclaw_action_summary_ref: `{}`",
            bundle.evidence_bundle.openclaw_action_summary_ref
        ),
        format!(
            "- report_command_ref: `{}`",
            bundle.evidence_bundle.report_command_ref
        ),
        format!(
            "- assembly_summary_ref: `{}`",
            bundle.evidence_bundle.assembly_summary_ref
        ),
        String::new(),
        "## Pinned runtime metadata".to_owned(),
        format!(
            "- openclaw_runtime_command: `{}`",
            bundle.runtime_metadata.openclaw.runtime_command
        ),
        format!(
            "- openclaw_version: `{}`",
            bundle
                .runtime_metadata
                .openclaw
                .version
                .as_deref()
                .unwrap_or("unknown")
        ),
        format!(
            "- openclaw_image_ref: `{}`",
            bundle
                .runtime_metadata
                .openclaw
                .image_ref
                .as_deref()
                .unwrap_or("unconfigured")
        ),
        format!(
            "- harness_version: `{}`",
            bundle.runtime_metadata.a2ex.harness_version
        ),
        format!(
            "- rust_version_requirement: `{}`",
            bundle.runtime_metadata.a2ex.rust_version_requirement
        ),
        format!(
            "- run_entrypoint: `{}`",
            bundle.runtime_metadata.a2ex.run_entrypoint
        ),
        format!(
            "- report_command_ref: `{}`",
            bundle.runtime_metadata.a2ex.report_command_ref
        ),
        format!(
            "- report_command_template: `{}`",
            bundle
                .runtime_metadata
                .a2ex
                .report_command_template
                .join(" ")
        ),
        format!(
            "- waiaas_base_url: `{}`",
            bundle
                .runtime_metadata
                .waiaas
                .base_url
                .as_deref()
                .unwrap_or("unconfigured")
        ),
        format!(
            "- waiaas_version: `{}`",
            bundle
                .runtime_metadata
                .waiaas
                .version
                .as_deref()
                .unwrap_or("unknown")
        ),
        String::new(),
        "## Canonical rereads".to_owned(),
        format!(
            "- operator-report: `{}`",
            bundle.canonical_rereads.operator_report.evidence_ref
        ),
        format!(
            "- report-window: `{}`",
            bundle.canonical_rereads.report_window.evidence_ref
        ),
        format!(
            "- exception-rollup: `{}`",
            bundle.canonical_rereads.exception_rollup.evidence_ref
        ),
        format!(
            "- runtime control status: `{}`",
            bundle.canonical_rereads.runtime_control_status.evidence_ref
        ),
        format!(
            "- runtime control failures: `{}`",
            bundle
                .canonical_rereads
                .runtime_control_failures
                .evidence_ref
        ),
        format!(
            "- canonical_reread_diagnostics: {}",
            if bundle.canonical_reread_collection.diagnostics.is_empty() {
                "none".to_owned()
            } else {
                bundle
                    .canonical_reread_collection
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.code.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        String::new(),
        "## OpenClaw action summary".to_owned(),
        format!(
            "- launch_mode: `{}`",
            bundle.openclaw_action_summary.launch_mode
        ),
        format!(
            "- overall_status: `{}`",
            bundle.openclaw_action_summary.overall_status
        ),
        format!(
            "- request_path: `{}`",
            bundle.openclaw_action_summary.request_path
        ),
        format!(
            "- guidance_path: `{}`",
            bundle.openclaw_action_summary.guidance_path
        ),
        format!(
            "- stdout_path: `{}`",
            bundle.openclaw_action_summary.stdout_path
        ),
        format!(
            "- stderr_path: `{}`",
            bundle.openclaw_action_summary.stderr_path
        ),
        String::new(),
        "## Verdict reasoning".to_owned(),
        format!(
            "- reasoning_summary: {}",
            bundle.final_classification.reasoning_summary
        ),
        format!(
            "- decisive_evidence_refs: {}",
            bundle
                .final_classification
                .decisive_evidence_refs
                .join(", ")
        ),
        format!(
            "- reasoning_evidence_refs: {}",
            bundle
                .final_classification
                .reasoning_evidence_refs
                .join(", ")
        ),
        format!(
            "- reread_snapshot_refs: {}",
            bundle.final_classification.reread_snapshot_refs.join(", ")
        ),
        format!(
            "- regenerated_from_persisted_facts: `{}`",
            bundle.final_classification.regenerated_from_persisted_facts
        ),
        format!(
            "- mismatch_diagnostics: {}",
            if bundle.final_classification.mismatch_diagnostics.is_empty() {
                "none".to_owned()
            } else {
                bundle
                    .final_classification
                    .mismatch_diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.code.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
    ];

    for section in &bundle.sections {
        markdown.push(String::new());
        markdown.push(format!("## {}", section.title));
        markdown.push(section.summary.clone());
        if !section.evidence_refs.is_empty() {
            markdown.push(String::new());
            for evidence_ref in &section.evidence_refs {
                markdown.push(format!("- `{evidence_ref}`"));
            }
        }
    }

    markdown.join("\n")
}
