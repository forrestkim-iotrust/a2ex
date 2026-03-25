use serde::{Deserialize, Serialize};

use crate::{
    a2ex_rereads::A2exCanonicalRereads,
    evidence_bundle::{CanonicalRereadRefs, EvidenceBundleRefs},
    run_state::{PreTradeOutcome, Verdict, WaiaasAuthorityState},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRouteVerdictState {
    pub attempt_decision: Verdict,
    pub reason_code: String,
    pub summary: String,
    pub decisive_signal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_chain_receipt_ref: Option<String>,
    pub command_configured: bool,
    pub command_completed: bool,
    pub execution_success: bool,
}

impl Default for LiveRouteVerdictState {
    fn default() -> Self {
        Self {
            attempt_decision: Verdict::Hold,
            reason_code: "awaiting_live_route_execution".to_owned(),
            summary: "approval or mutation receipts alone cannot mark the bounded live route green"
                .to_owned(),
            decisive_signal: "destination-chain USDC receipt on Base".to_owned(),
            evidence_ref: Some("live-route-evidence.json".to_owned()),
            destination_chain_receipt_ref: None,
            command_configured: false,
            command_completed: false,
            execution_success: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictMismatchDiagnostic {
    pub code: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictClassifierInput {
    pub canonical_rereads: A2exCanonicalRereads,
    pub canonical_reread_refs: CanonicalRereadRefs,
    pub waiaas_authority: WaiaasAuthorityState,
    pub live_route: LiveRouteVerdictState,
    pub evidence_bundle: EvidenceBundleRefs,
    pub pre_trade_outcome: PreTradeOutcome,
    pub regenerated_from_persisted_facts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedVerdict {
    pub verdict: Verdict,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisive_evidence_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisive_evidence_refs: Vec<String>,
    pub summary: String,
    pub reasoning_summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reread_snapshot_refs: Vec<String>,
    pub regenerated_from_persisted_facts: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mismatch_diagnostics: Vec<VerdictMismatchDiagnostic>,
}

pub fn classify_verdict(input: &VerdictClassifierInput) -> ClassifiedVerdict {
    let reread_snapshot_refs = input.canonical_reread_refs.all_refs();
    let mismatch_diagnostics = collect_mismatch_diagnostics(input);
    let live_route_ref = input.live_route.evidence_ref.clone();
    let authority_ref = input.waiaas_authority.evidence_ref.clone();
    let runtime_failures_ref = Some(
        input
            .canonical_reread_refs
            .runtime_control_failures
            .evidence_ref
            .clone(),
    );

    if !mismatch_diagnostics.is_empty() {
        return build(
            Verdict::Fail,
            "canonical_reread_mismatch",
            live_route_ref.clone().or_else(|| authority_ref.clone()),
            "canonical rereads disagree about hold/failure/rejection state, so the final verdict cannot trust a single surface",
            evidence_refs(
                &reread_snapshot_refs,
                &[authority_ref, live_route_ref, runtime_failures_ref],
            ),
            reread_snapshot_refs,
            input.regenerated_from_persisted_facts,
            mismatch_diagnostics,
        );
    }

    if !input.canonical_rereads.diagnostics.is_empty()
        && input.pre_trade_outcome != PreTradeOutcome::Blocked
        && input.live_route.attempt_decision != Verdict::Pass
    {
        let summary = input
            .canonical_rereads
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.summary.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return build(
            Verdict::Hold,
            "canonical_rereads_incomplete",
            Some(
                input
                    .canonical_reread_refs
                    .operator_report
                    .evidence_ref
                    .clone(),
            ),
            format!(
                "canonical rereads are incomplete, so the verdict remains hold until operator/runtime truth can be reread safely: {summary}"
            ),
            evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
            reread_snapshot_refs,
            input.regenerated_from_persisted_facts,
            Vec::new(),
        );
    }

    match input.pre_trade_outcome {
        PreTradeOutcome::Blocked => {
            return build(
                Verdict::Blocked,
                "canonical_pretrade_blocked",
                Some(
                    input
                        .canonical_reread_refs
                        .operator_report
                        .evidence_ref
                        .clone(),
                ),
                "canonical A2EX rereads show the pre-trade path is blocked before live-route success criteria can be satisfied",
                evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
                reread_snapshot_refs,
                input.regenerated_from_persisted_facts,
                Vec::new(),
            );
        }
        PreTradeOutcome::Pending | PreTradeOutcome::Incomplete => {
            return build(
                Verdict::Hold,
                "approval_boundary_not_reached",
                Some(
                    input
                        .canonical_reread_refs
                        .operator_report
                        .evidence_ref
                        .clone(),
                ),
                "canonical A2EX rereads have not yet reached an approved selection, so the bounded live route cannot be considered pass or fail",
                evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
                reread_snapshot_refs,
                input.regenerated_from_persisted_facts,
                Vec::new(),
            );
        }
        PreTradeOutcome::ApprovalBoundaryReached => {}
    }

    match input.waiaas_authority.authority_decision {
        crate::run_state::AuthorityDecision::Blocked => {
            return build(
                Verdict::Blocked,
                input.waiaas_authority.reason_code.clone(),
                authority_ref.clone(),
                "WAIaaS session or policy authority blocked the live attempt before route execution",
                evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
                reread_snapshot_refs,
                input.regenerated_from_persisted_facts,
                Vec::new(),
            );
        }
        crate::run_state::AuthorityDecision::Fail => {
            return build(
                Verdict::Fail,
                input.waiaas_authority.reason_code.clone(),
                authority_ref.clone(),
                "WAIaaS authority inspection failed before the bounded live route could start",
                evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
                reread_snapshot_refs,
                input.regenerated_from_persisted_facts,
                Vec::new(),
            );
        }
        crate::run_state::AuthorityDecision::Hold => {
            return build(
                Verdict::Hold,
                input.waiaas_authority.reason_code.clone(),
                authority_ref.clone(),
                "WAIaaS authority reported a recoverable hold before route execution",
                evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
                reread_snapshot_refs,
                input.regenerated_from_persisted_facts,
                Vec::new(),
            );
        }
        crate::run_state::AuthorityDecision::Pass => {}
    }

    if let Some(exception_rollup) = &input.canonical_rereads.exception_rollup {
        if let Some(active_hold) = &exception_rollup.active_hold {
            return build(
                Verdict::Hold,
                active_hold.reason_code.as_str(),
                Some(
                    input
                        .canonical_reread_refs
                        .exception_rollup
                        .evidence_ref
                        .clone(),
                ),
                format!(
                    "canonical exception-rollup shows an active hold even after approval boundary: {}",
                    active_hold.summary
                ),
                evidence_refs(
                    &reread_snapshot_refs,
                    &[live_route_ref, runtime_failures_ref],
                ),
                reread_snapshot_refs,
                input.regenerated_from_persisted_facts,
                Vec::new(),
            );
        }
    }

    match input.live_route.attempt_decision {
        Verdict::Pass => build(
            Verdict::Pass,
            input.live_route.reason_code.clone(),
            input
                .live_route
                .destination_chain_receipt_ref
                .clone()
                .or_else(|| live_route_ref.clone()),
            "canonical rereads, WAIaaS authority, and live-route evidence agree that decisive destination evidence was captured",
            evidence_refs(
                &reread_snapshot_refs,
                &[
                    authority_ref,
                    live_route_ref,
                    input.live_route.destination_chain_receipt_ref.clone(),
                ],
            ),
            reread_snapshot_refs,
            input.regenerated_from_persisted_facts,
            Vec::new(),
        ),
        Verdict::Fail => build(
            Verdict::Fail,
            input.live_route.reason_code.clone(),
            live_route_ref.clone(),
            input.live_route.summary.clone(),
            evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
            reread_snapshot_refs,
            input.regenerated_from_persisted_facts,
            Vec::new(),
        ),
        Verdict::Blocked => build(
            Verdict::Blocked,
            input.live_route.reason_code.clone(),
            live_route_ref.clone(),
            input.live_route.summary.clone(),
            evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
            reread_snapshot_refs,
            input.regenerated_from_persisted_facts,
            Vec::new(),
        ),
        Verdict::Hold => build(
            Verdict::Hold,
            input.live_route.reason_code.clone(),
            live_route_ref.clone(),
            input.live_route.summary.clone(),
            evidence_refs(&reread_snapshot_refs, &[authority_ref, live_route_ref]),
            reread_snapshot_refs,
            input.regenerated_from_persisted_facts,
            Vec::new(),
        ),
    }
}

fn collect_mismatch_diagnostics(input: &VerdictClassifierInput) -> Vec<VerdictMismatchDiagnostic> {
    let mut diagnostics = Vec::new();
    let refs = &input.canonical_reread_refs;

    if let (Some(operator_report), Some(report_window)) = (
        input.canonical_rereads.operator_report.as_ref(),
        input.canonical_rereads.report_window.as_ref(),
    ) {
        if operator_report.hold_reason != report_window.current_operator_report.hold_reason {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "operator_report_hold_reason_mismatch".to_owned(),
                summary: "operator-report and report-window.current_operator_report disagree about the active hold reason".to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.report_window.evidence_ref.clone(),
                ],
            });
        }
        if operator_report.last_runtime_failure
            != report_window.current_operator_report.last_runtime_failure
        {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "operator_report_failure_mismatch".to_owned(),
                summary: "operator-report and report-window.current_operator_report disagree about the last runtime failure".to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.report_window.evidence_ref.clone(),
                ],
            });
        }
        if operator_report.last_runtime_rejection
            != report_window.current_operator_report.last_runtime_rejection
        {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "operator_report_rejection_mismatch".to_owned(),
                summary: "operator-report and report-window.current_operator_report disagree about the last runtime rejection".to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.report_window.evidence_ref.clone(),
                ],
            });
        }
        if operator_report.control_mode != report_window.current_operator_report.control_mode {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "operator_report_control_mode_mismatch".to_owned(),
                summary: "operator-report and report-window.current_operator_report disagree about runtime control mode".to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.report_window.evidence_ref.clone(),
                ],
            });
        }
    }

    if let (Some(operator_report), Some(exception_rollup)) = (
        input.canonical_rereads.operator_report.as_ref(),
        input.canonical_rereads.exception_rollup.as_ref(),
    ) {
        if operator_report.hold_reason
            != exception_rollup
                .active_hold
                .as_ref()
                .map(|hold| hold.reason_code.clone())
        {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "operator_report_exception_hold_mismatch".to_owned(),
                summary:
                    "operator-report and exception-rollup disagree about the active hold reason"
                        .to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.exception_rollup.evidence_ref.clone(),
                ],
            });
        }
        if operator_report.last_runtime_failure != exception_rollup.last_runtime_failure {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "operator_report_exception_failure_mismatch".to_owned(),
                summary:
                    "operator-report and exception-rollup disagree about the last runtime failure"
                        .to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.exception_rollup.evidence_ref.clone(),
                ],
            });
        }
        if operator_report.last_runtime_rejection != exception_rollup.last_runtime_rejection {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "operator_report_exception_rejection_mismatch".to_owned(),
                summary:
                    "operator-report and exception-rollup disagree about the last runtime rejection"
                        .to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.exception_rollup.evidence_ref.clone(),
                ],
            });
        }
    }

    if let (Some(operator_report), Some(runtime_status)) = (
        input.canonical_rereads.operator_report.as_ref(),
        input.canonical_rereads.runtime_control_status.as_ref(),
    ) {
        if operator_report.control_mode != runtime_status.control_mode {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "runtime_control_status_mismatch".to_owned(),
                summary: "operator-report control_mode disagrees with runtime control status"
                    .to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.runtime_control_status.evidence_ref.clone(),
                ],
            });
        }
    }

    if let (Some(operator_report), Some(runtime_failures)) = (
        input.canonical_rereads.operator_report.as_ref(),
        input.canonical_rereads.runtime_control_failures.as_ref(),
    ) {
        let runtime_rejection = runtime_failures.last_rejection.as_ref().map(|rejection| {
            (
                rejection.code.clone(),
                rejection.message.clone(),
                rejection.rejected_at.clone(),
            )
        });
        let operator_rejection = operator_report
            .last_runtime_rejection
            .as_ref()
            .map(|rejection| {
                (
                    rejection.code.clone(),
                    rejection.message.clone(),
                    rejection.observed_at.clone(),
                )
            });
        if runtime_rejection != operator_rejection {
            diagnostics.push(VerdictMismatchDiagnostic {
                code: "runtime_control_failures_mismatch".to_owned(),
                summary:
                    "operator-report last_runtime_rejection disagrees with runtime control failures"
                        .to_owned(),
                evidence_refs: vec![
                    refs.operator_report.evidence_ref.clone(),
                    refs.runtime_control_failures.evidence_ref.clone(),
                ],
            });
        }
    }

    diagnostics
}

fn evidence_refs(reread_snapshot_refs: &[String], extras: &[Option<String>]) -> Vec<String> {
    let mut refs = reread_snapshot_refs.to_vec();
    for extra in extras.iter().flatten() {
        if !refs.contains(extra) {
            refs.push(extra.clone());
        }
    }
    refs
}

fn build(
    verdict: Verdict,
    reason_code: impl Into<String>,
    decisive_evidence_ref: Option<String>,
    summary: impl Into<String>,
    reasoning_evidence_refs: Vec<String>,
    reread_snapshot_refs: Vec<String>,
    regenerated_from_persisted_facts: bool,
    mismatch_diagnostics: Vec<VerdictMismatchDiagnostic>,
) -> ClassifiedVerdict {
    let reason_code = reason_code.into();
    let summary = summary.into();
    let mut decisive_evidence_refs = decisive_evidence_ref.iter().cloned().collect::<Vec<_>>();
    if decisive_evidence_refs.is_empty() {
        decisive_evidence_refs = reread_snapshot_refs.clone();
    }
    ClassifiedVerdict {
        verdict,
        reason_code,
        decisive_evidence_ref,
        decisive_evidence_refs,
        summary: summary.clone(),
        reasoning_summary: summary,
        reasoning_evidence_refs,
        reread_snapshot_refs,
        regenerated_from_persisted_facts,
        mismatch_diagnostics,
    }
}
