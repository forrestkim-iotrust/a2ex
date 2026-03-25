use std::{
    env,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
};

use a2ex_live_harness::{
    ASSEMBLY_REPORT_COMMAND_REF, ASSEMBLY_SUMMARY_FILE_NAME, AssemblyFailureDiagnostic,
    AssemblySummary, AssemblyVerificationStatus, AuthorityDecision, ErrorClass, HarnessPhase,
    HarnessRunState, HttpWaiaasAuthorityAdapter, LaunchMetadata, LiveAttemptPhase,
    OpenClawActionSummary, OpenClawLaunchMode, OpenClawLaunchRequest, PreTradeOutcome,
    PreflightConfig, Verdict, WaiaasAuthorityAdapter, WaiaasAuthorityRequest, WaiaasInspection,
    collect_canonical_rereads, default_guidance_contract, highest_observed_phase,
    persist_action_summary, render_evidence_summary_markdown, reread_canonical_state,
    run_preflight,
};
use a2ex_onboarding::StrategyRuntimeHoldReason;
use a2ex_state::{AUTONOMOUS_RUNTIME_CONTROL_SCOPE, PersistedRuntimeControl, StateRepository};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

const LIVE_ROUTE_EVIDENCE_REF: &str = "live-route-evidence.json";
const LIVE_ROUTE_STDOUT_REF: &str = "live-route.stdout.log";
const LIVE_ROUTE_STDERR_REF: &str = "live-route.stderr.log";
const S03_EVIDENCE_BUNDLE_REF: &str = "evidence-bundle.json";
const S03_EVIDENCE_SUMMARY_REF: &str = "evidence-summary.md";
const S03_OPENCLAW_ACTION_SUMMARY_REF: &str = "openclaw-action-summary.json";
const S03_REPORT_COMMAND_REF: &str = "scripts/report-m008-s03.sh";
// S04 persists assembly-summary.json and later rereads it through scripts/report-m008-s04.sh.
const S04_REPORT_COMMAND_REF: &str = ASSEMBLY_REPORT_COMMAND_REF;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("a2ex-live-harness: {error}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), String> {
    let harness = HarnessEnv::from_env()?;
    let preflight = run_preflight(
        &harness.preflight_config,
        &a2ex_live_harness::LocalPreflightProbe,
    )
    .await;
    let mut run_state = HarnessRunState::new(preflight);
    if let Some(run_id_override) = &harness.run_id_override {
        run_state.run_id = run_id_override.clone();
    }
    run_state.set_install_url(harness.install_url.clone());
    run_state.set_waiaas(WaiaasInspection {
        base_url: run_state.preflight.runtime_metadata.waiaas.base_url.clone(),
        health_url: run_state
            .preflight
            .runtime_metadata
            .waiaas
            .health_url
            .clone(),
        session_url: run_state
            .preflight
            .runtime_metadata
            .waiaas
            .session_url
            .clone(),
        policy_url: run_state
            .preflight
            .runtime_metadata
            .waiaas
            .policy_url
            .clone(),
        session_id: harness.preflight_config.waiaas_session_id.clone(),
        policy_id: harness.preflight_config.waiaas_policy_id.clone(),
    });

    let run_state_path = harness.run_state_path(&run_state.run_id);
    let run_dir = run_state_path
        .parent()
        .ok_or_else(|| "run state path must have a parent".to_owned())?
        .to_path_buf();
    let mut openclaw_action_summary: Option<OpenClawActionSummary> = None;

    run_state.launch.run_state_path = Some(run_state_path.display().to_string());
    run_state.persist(&run_state_path).map_err(io_err)?;

    if !run_state.preflight.is_ok() {
        run_state.mark_error(
            run_state
                .last_error_class
                .clone()
                .unwrap_or_else(|| ErrorClass::PrerequisiteMissing.to_string()),
            "preflight blocked OpenClaw launch",
        );
        persist_evidence_artifacts(&run_dir, &mut run_state, openclaw_action_summary.as_ref())
            .map_err(io_err)?;
        run_state.persist(&run_state_path).map_err(io_err)?;
        print_summary(&run_state, &run_state_path);
        return Err("preflight failed; inspect persisted run_state.json".to_owned());
    }

    let launch_request = OpenClawLaunchRequest {
        run_id: run_state.run_id.clone(),
        install_url: harness.install_url.clone(),
        goal: harness.goal.clone(),
        runtime_command: harness.preflight_config.openclaw_runtime_command.clone(),
        image_ref: harness.preflight_config.openclaw_image_ref.clone(),
        launch_mode: harness.launch_mode.clone(),
        mcp_spawn_command: run_state
            .preflight
            .runtime_metadata
            .a2ex
            .mcp_spawn_command
            .clone(),
        guidance_contract: default_guidance_contract(),
        work_dir: harness.repo_root.clone(),
    };

    run_state.transition(
        HarnessPhase::OpenclawLaunchPending,
        "launch contract written; waiting for OpenClaw to own the install→approval loop",
    );
    run_state.persist(&run_state_path).map_err(io_err)?;

    let mut handle = launch_request
        .clone()
        .launch(&run_dir)
        .await
        .map_err(io_err)?;
    let openclaw_mode = match &handle.mode {
        OpenClawLaunchMode::Spawn => "spawn",
        OpenClawLaunchMode::Attach => "attach",
    }
    .to_owned();
    run_state.set_launch_metadata(LaunchMetadata {
        openclaw_pid: handle.pid,
        openclaw_mode: Some(openclaw_mode.clone()),
        openclaw_request_path: Some(handle.artifacts.request_path.display().to_string()),
        openclaw_guidance_path: Some(handle.artifacts.guidance_path.display().to_string()),
        openclaw_stdout_path: Some(handle.artifacts.stdout_path.display().to_string()),
        openclaw_stderr_path: Some(handle.artifacts.stderr_path.display().to_string()),
        mcp_spawn_command: run_state
            .preflight
            .runtime_metadata
            .a2ex
            .mcp_spawn_command
            .clone(),
        run_state_path: Some(run_state_path.display().to_string()),
    });
    if matches!(&handle.mode, OpenClawLaunchMode::Attach) {
        run_state.transition(
            HarnessPhase::OpenclawAttached,
            "attach mode requested; OpenClaw launch responsibility stays with the external runtime",
        );
    }
    run_state.transition(
        HarnessPhase::InstallBootstrapPending,
        format!(
            "OpenClaw {openclaw_mode} prepared; expected next step is onboarding.bootstrap_install via a2ex-mcp stdio"
        ),
    );
    run_state.persist(&run_state_path).map_err(io_err)?;

    let exit = handle.wait().await.map_err(io_err)?;
    openclaw_action_summary = Some(
        persist_action_summary(
            &launch_request,
            &handle.artifacts,
            &handle.mode,
            &exit,
            &run_dir,
        )
        .map_err(io_err)?,
    );
    apply_canonical_reread(&harness, &mut run_state)?;
    collect_and_apply_a2ex_rereads(&harness, &mut run_state).await;
    advance_phase_history(&mut run_state);

    if !exit.success {
        run_state.mark_error(
            ErrorClass::OpenclawUnavailable.to_string(),
            format!(
                "OpenClaw exited before the approval boundary: {:?}",
                exit.status_code
            ),
        );
        persist_evidence_artifacts(&run_dir, &mut run_state, openclaw_action_summary.as_ref())
            .map_err(io_err)?;
        run_state.persist(&run_state_path).map_err(io_err)?;
        print_summary(&run_state, &run_state_path);
        return Err("OpenClaw exited unsuccessfully; inspect run_state and logs".to_owned());
    }

    match run_state.pre_trade_outcome {
        PreTradeOutcome::ApprovalBoundaryReached => {
            run_state.transition(
                HarnessPhase::ApprovalBoundaryReached,
                "canonical state.db reread found an approved selection; handing off into the checked-in S02 live route",
            );
            inspect_and_persist_waiaas_authority(&harness, &run_dir, &mut run_state)
                .await
                .map_err(io_err)?;
            collect_and_apply_a2ex_rereads(&harness, &mut run_state).await;
            execute_live_route(&harness, &run_dir, &mut run_state)
                .await
                .map_err(io_err)?;
            collect_and_apply_a2ex_rereads(&harness, &mut run_state).await;
        }
        PreTradeOutcome::Blocked => {
            run_state.transition(
                highest_observed_phase(&run_state),
                "canonical state.db reread shows a blocked pre-trade outcome; inspect run_state.json for the last completed phase and canonical statuses",
            );
        }
        PreTradeOutcome::Incomplete | PreTradeOutcome::Pending => {
            run_state.mark_incomplete(
                "OpenClaw exited without a blocked state or approved selection; canonical ids remain incomplete for this pre-trade run",
            );
        }
    }
    // S04 freezes the decisive snapshot before any later report-m008-s04.sh,
    // runtime_stop_status, or runtime_clear_stop_status verification can mutate the live verdict.
    run_state.freeze_decisive_snapshot(now_unix_timestamp());
    perform_post_control_verification(&harness, &mut run_state)
        .await
        .map_err(io_err)?;
    persist_evidence_artifacts(&run_dir, &mut run_state, openclaw_action_summary.as_ref())
        .map_err(io_err)?;
    run_state.persist(&run_state_path).map_err(io_err)?;
    print_summary(&run_state, &run_state_path);
    Ok(())
}

fn apply_canonical_reread(
    harness: &HarnessEnv,
    run_state: &mut HarnessRunState,
) -> Result<(), String> {
    let reread = reread_canonical_state(&harness.state_db_path, &harness.install_url)
        .map_err(|error| format!("canonical state reread failed: {error}"))?;
    run_state.apply_canonical_reread(reread);
    Ok(())
}

async fn collect_and_apply_a2ex_rereads(harness: &HarnessEnv, run_state: &mut HarnessRunState) {
    let rereads = collect_canonical_rereads(
        &harness.state_db_path,
        run_state.install_id.as_deref(),
        run_state.proposal_id.as_deref(),
        run_state.selection_id.as_deref(),
        &run_state.canonical_rereads,
    )
    .await;
    if rereads
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code.contains("mismatch"))
    {
        run_state.last_error_class = Some(ErrorClass::CanonicalTruthMismatch.as_str().to_owned());
    } else if !rereads.diagnostics.is_empty() {
        run_state.last_error_class =
            Some(ErrorClass::A2exCanonicalRereadMissing.as_str().to_owned());
    }
    run_state.set_canonical_reread_collection(rereads, true);
}

async fn inspect_and_persist_waiaas_authority(
    harness: &HarnessEnv,
    run_dir: &Path,
    run_state: &mut HarnessRunState,
) -> std::io::Result<()> {
    let Some(request) = harness.waiaas_authority_request() else {
        return Ok(());
    };

    let adapter = HttpWaiaasAuthorityAdapter::default();
    let outcome = adapter.inspect(&request).await;
    let capture = outcome.capture().clone();
    capture.persist(&run_dir.join(&capture.evidence_ref))?;
    run_state.apply_waiaas_authority(&outcome);
    Ok(())
}

async fn execute_live_route(
    harness: &HarnessEnv,
    run_dir: &Path,
    run_state: &mut HarnessRunState,
) -> std::io::Result<()> {
    let mut evidence = LiveRouteEvidence::from_run_state(run_state);
    run_state.evidence_refs.live_route_evidence.evidence_ref =
        Some(LIVE_ROUTE_EVIDENCE_REF.to_owned());

    match run_state.waiaas_authority.authority_decision {
        AuthorityDecision::Blocked => {
            evidence.attempt_decision = Verdict::Blocked;
            evidence.reason_code = run_state.live_route_result.reason_code.clone();
            evidence.summary = run_state.live_route_result.summary.clone();
        }
        AuthorityDecision::Hold => {
            evidence.attempt_decision = Verdict::Hold;
            evidence.reason_code = run_state.live_route_result.reason_code.clone();
            evidence.summary = run_state.live_route_result.summary.clone();
        }
        AuthorityDecision::Fail => {
            evidence.attempt_decision = Verdict::Fail;
            evidence.reason_code = run_state.live_route_result.reason_code.clone();
            evidence.summary = run_state.live_route_result.summary.clone();
        }
        AuthorityDecision::Pass => {
            evidence.execution = execute_live_route_command(harness, run_dir).await?;
            run_state.live_route_result.command_configured = evidence.execution.command.is_some();
            run_state.live_route_result.command_completed =
                evidence.execution.finished_at.is_some();
            run_state.live_route_result.execution_success = evidence.execution.success;
            if let Some(spawn_error) = evidence.execution.spawn_error.clone() {
                run_state.last_error_class = Some("live_route_execution_failed".to_owned());
                run_state.set_live_attempt_phase(
                    LiveAttemptPhase::Failed,
                    format!("bounded live route failed to launch: {spawn_error}"),
                );
                run_state.live_route_result.attempt_decision = Verdict::Fail;
                run_state.live_route_result.reason_code =
                    "live_route_command_spawn_failed".to_owned();
                run_state.live_route_result.summary =
                    "bounded live route command could not be started after WAIaaS authority pass"
                        .to_owned();
                evidence.attempt_decision = Verdict::Fail;
                evidence.reason_code = "live_route_command_spawn_failed".to_owned();
                evidence.summary = run_state.live_route_result.summary.clone();
            } else if evidence.execution.command.is_none() {
                run_state.set_live_attempt_phase(
                    LiveAttemptPhase::Hold,
                    "WAIaaS authority passed, but no bounded live route command is configured yet",
                );
                run_state.live_route_result.attempt_decision = Verdict::Hold;
                run_state.live_route_result.reason_code =
                    "live_route_command_not_configured".to_owned();
                run_state.live_route_result.summary =
                    "WAIaaS authority is present, but the bounded live route command is not configured"
                        .to_owned();
                evidence.attempt_decision = Verdict::Hold;
                evidence.reason_code = "live_route_command_not_configured".to_owned();
                evidence.summary = run_state.live_route_result.summary.clone();
            } else if !evidence.execution.success {
                run_state.last_error_class = Some("live_route_execution_failed".to_owned());
                run_state.set_live_attempt_phase(
                    LiveAttemptPhase::Failed,
                    format!(
                        "bounded live route command exited unsuccessfully: {:?}",
                        evidence.execution.status_code
                    ),
                );
                run_state.live_route_result.attempt_decision = Verdict::Fail;
                run_state.live_route_result.reason_code = "live_route_command_failed".to_owned();
                run_state.live_route_result.summary =
                    "bounded live route command exited unsuccessfully before destination confirmation"
                        .to_owned();
                evidence.attempt_decision = Verdict::Fail;
                evidence.reason_code = "live_route_command_failed".to_owned();
                evidence.summary = run_state.live_route_result.summary.clone();
            } else {
                run_state.set_live_attempt_phase(
                    LiveAttemptPhase::DestinationConfirmationPending,
                    "bounded live route command completed; waiting for decisive destination-chain USDC receipt evidence",
                );

                if let Some(receipt) = harness.destination_receipt() {
                    run_state.set_live_attempt_phase(
                        LiveAttemptPhase::Completed,
                        "bounded live route completed with destination-chain USDC receipt evidence on Base",
                    );
                    run_state.live_route_result.attempt_decision = Verdict::Pass;
                    run_state.live_route_result.reason_code =
                        "live_route_destination_receipt_confirmed".to_owned();
                    run_state.live_route_result.summary =
                        "bounded live route completed with decisive destination-chain USDC receipt evidence on Base"
                            .to_owned();
                    run_state.live_route_result.destination_chain_receipt_ref =
                        Some(receipt.receipt_ref.clone());
                    evidence.destination_chain_receipt_ref = Some(receipt.receipt_ref.clone());
                    evidence.destination_chain_receipt = Some(receipt);
                    evidence.attempt_decision = Verdict::Pass;
                    evidence.reason_code = "live_route_destination_receipt_confirmed".to_owned();
                    evidence.summary = run_state.live_route_result.summary.clone();
                } else {
                    run_state.set_live_attempt_phase(
                        LiveAttemptPhase::Hold,
                        "bounded live route command completed, but destination-chain receipt evidence is still missing",
                    );
                    run_state.live_route_result.attempt_decision = Verdict::Hold;
                    run_state.live_route_result.reason_code =
                        "destination_confirmation_pending".to_owned();
                    run_state.live_route_result.summary =
                        "bounded live route command completed, but decisive destination-chain USDC receipt evidence is still missing"
                            .to_owned();
                    evidence.attempt_decision = Verdict::Hold;
                    evidence.reason_code = "destination_confirmation_pending".to_owned();
                    evidence.summary = run_state.live_route_result.summary.clone();
                }
            }
        }
    }

    run_state.recompute_final_classification(false);
    evidence.persist(&run_dir.join(LIVE_ROUTE_EVIDENCE_REF))
}

async fn perform_post_control_verification(
    harness: &HarnessEnv,
    run_state: &mut HarnessRunState,
) -> std::io::Result<()> {
    let mut diagnostics = Vec::new();

    let report_checked_at = now_unix_timestamp();
    let report_rereads = collect_canonical_rereads(
        &harness.state_db_path,
        run_state.install_id.as_deref(),
        run_state.proposal_id.as_deref(),
        run_state.selection_id.as_deref(),
        &run_state.canonical_rereads,
    )
    .await;
    let report_status = if report_rereads.collected_successfully() {
        AssemblyVerificationStatus::Pass
    } else {
        diagnostics.extend(assembly_diagnostics_from_rereads(
            "post_control_report_reread_failed",
            &report_checked_at,
            &report_rereads,
        ));
        AssemblyVerificationStatus::Fail
    };

    let runtime_stop_checked_at = now_unix_timestamp();
    let runtime_stop_status =
        match apply_runtime_stop(&harness.state_db_path, &runtime_stop_checked_at).await {
            Ok(_) => {
                let rereads = collect_canonical_rereads(
                    &harness.state_db_path,
                    run_state.install_id.as_deref(),
                    run_state.proposal_id.as_deref(),
                    run_state.selection_id.as_deref(),
                    &run_state.canonical_rereads,
                )
                .await;
                verify_runtime_stop_rereads(&rereads, &runtime_stop_checked_at, &mut diagnostics)
            }
            Err(error) => {
                diagnostics.push(AssemblyFailureDiagnostic {
                    code: "runtime_stop_verification_failed".to_owned(),
                    summary: format!("runtime.stop mutation failed: {error}"),
                    evidence_refs: vec![
                        run_state
                            .canonical_rereads
                            .runtime_control_status
                            .evidence_ref
                            .clone(),
                        run_state
                            .canonical_rereads
                            .runtime_control_failures
                            .evidence_ref
                            .clone(),
                    ],
                    observed_at: runtime_stop_checked_at.clone(),
                });
                AssemblyVerificationStatus::Fail
            }
        };

    let runtime_clear_stop_checked_at = now_unix_timestamp();
    let runtime_clear_stop_status = match apply_runtime_clear_stop(
        &harness.state_db_path,
        &runtime_clear_stop_checked_at,
    )
    .await
    {
        Ok(_) => {
            let rereads = collect_canonical_rereads(
                &harness.state_db_path,
                run_state.install_id.as_deref(),
                run_state.proposal_id.as_deref(),
                run_state.selection_id.as_deref(),
                &run_state.canonical_rereads,
            )
            .await;
            verify_runtime_clear_stop_rereads(
                &rereads,
                &runtime_clear_stop_checked_at,
                &mut diagnostics,
            )
        }
        Err(error) => {
            diagnostics.push(AssemblyFailureDiagnostic {
                code: "runtime_clear_stop_verification_failed".to_owned(),
                summary: format!("runtime.clear_stop mutation failed: {error}"),
                evidence_refs: vec![
                    run_state
                        .canonical_rereads
                        .runtime_control_status
                        .evidence_ref
                        .clone(),
                    run_state
                        .canonical_rereads
                        .runtime_control_failures
                        .evidence_ref
                        .clone(),
                ],
                observed_at: runtime_clear_stop_checked_at.clone(),
            });
            AssemblyVerificationStatus::Fail
        }
    };

    let mismatch_checked_at = now_unix_timestamp();
    let mismatch_status = if run_state.decisive_snapshot.verdict
        == run_state.final_classification.verdict
        && run_state.decisive_snapshot.reason_code == run_state.final_classification.reason_code
        && run_state.decisive_snapshot.run_id == run_state.run_id
    {
        AssemblyVerificationStatus::Pass
    } else {
        diagnostics.push(AssemblyFailureDiagnostic {
            code: "snapshot_vs_post_control_mismatch".to_owned(),
            summary: format!(
                "decisive snapshot drifted after post-control verification: snapshot={:?}/{} current={:?}/{}",
                run_state.decisive_snapshot.verdict,
                run_state.decisive_snapshot.reason_code,
                run_state.final_classification.verdict,
                run_state.final_classification.reason_code,
            ),
            evidence_refs: vec![
                run_state.assembly_summary.summary_ref.clone(),
                run_state.decisive_snapshot.evidence_bundle_ref.clone(),
                run_state
                    .decisive_snapshot
                    .live_route_evidence_ref
                    .clone()
                    .unwrap_or_else(|| LIVE_ROUTE_EVIDENCE_REF.to_owned()),
            ],
            observed_at: mismatch_checked_at.clone(),
        });
        AssemblyVerificationStatus::Fail
    };

    run_state.note_post_control_verification(
        report_status,
        runtime_stop_status,
        runtime_clear_stop_status,
        mismatch_status,
        diagnostics,
    );
    run_state.post_control_verification.report_checked_at = Some(report_checked_at);
    run_state.post_control_verification.runtime_stop_checked_at = Some(runtime_stop_checked_at);
    run_state
        .post_control_verification
        .runtime_clear_stop_checked_at = Some(runtime_clear_stop_checked_at);
    run_state.post_control_verification.mismatch_checked_at = Some(mismatch_checked_at);
    Ok(())
}

fn assembly_diagnostics_from_rereads(
    code_prefix: &str,
    observed_at: &str,
    rereads: &a2ex_live_harness::A2exCanonicalRereads,
) -> Vec<AssemblyFailureDiagnostic> {
    rereads
        .diagnostics
        .iter()
        .map(|diagnostic| AssemblyFailureDiagnostic {
            code: format!("{code_prefix}:{}", diagnostic.code),
            summary: diagnostic.summary.clone(),
            evidence_refs: diagnostic.evidence_refs.clone(),
            observed_at: observed_at.to_owned(),
        })
        .collect()
}

fn verify_runtime_stop_rereads(
    rereads: &a2ex_live_harness::A2exCanonicalRereads,
    observed_at: &str,
    diagnostics: &mut Vec<AssemblyFailureDiagnostic>,
) -> AssemblyVerificationStatus {
    diagnostics.extend(assembly_diagnostics_from_rereads(
        "runtime_stop_verification_failed",
        observed_at,
        rereads,
    ));

    let operator_hold_ok = rereads
        .operator_report
        .as_ref()
        .and_then(|report| report.hold_reason)
        == Some(StrategyRuntimeHoldReason::RuntimeControlStopped);
    let report_window_hold_ok = rereads
        .report_window
        .as_ref()
        .and_then(|window| window.current_operator_report.hold_reason)
        == Some(StrategyRuntimeHoldReason::RuntimeControlStopped);
    let exception_hold_ok = rereads
        .exception_rollup
        .as_ref()
        .and_then(|rollup| rollup.active_hold.as_ref())
        .map(|hold| hold.reason_code)
        == Some(StrategyRuntimeHoldReason::RuntimeControlStopped);
    let control_status_ok = rereads
        .runtime_control_status
        .as_ref()
        .map(|status| status.control_mode == "stopped" && status.autonomy_eligibility == "blocked")
        .unwrap_or(false);
    let rejection_ok = rereads
        .runtime_control_failures
        .as_ref()
        .and_then(|failures| failures.last_rejection.as_ref())
        .map(|rejection| rejection.code == "runtime_stopped")
        .unwrap_or(false);

    if !operator_hold_ok {
        diagnostics.push(stage_failure(
            "runtime_stop_verification_failed",
            "operator-report reread did not expose hold_reason=runtime_control_stopped after runtime.stop",
            vec!["operator-report".to_owned()],
            observed_at,
        ));
    }
    if !report_window_hold_ok {
        diagnostics.push(stage_failure(
            "runtime_stop_verification_failed",
            "report-window reread did not expose hold_reason=runtime_control_stopped after runtime.stop",
            vec!["report-window".to_owned()],
            observed_at,
        ));
    }
    if !exception_hold_ok {
        diagnostics.push(stage_failure(
            "runtime_stop_verification_failed",
            "exception-rollup reread did not expose active_hold.reason_code=runtime_control_stopped after runtime.stop",
            vec!["exception-rollup".to_owned()],
            observed_at,
        ));
    }
    if !control_status_ok {
        diagnostics.push(stage_failure(
            "runtime_stop_verification_failed",
            "runtime control status reread did not expose control_mode=stopped and autonomy_eligibility=blocked after runtime.stop",
            vec!["runtime control status".to_owned()],
            observed_at,
        ));
    }
    if !rejection_ok {
        diagnostics.push(stage_failure(
            "runtime_stop_verification_failed",
            "runtime control failures reread did not preserve last_rejection.code=runtime_stopped after runtime.stop",
            vec!["runtime control failures".to_owned()],
            observed_at,
        ));
    }

    if operator_hold_ok
        && report_window_hold_ok
        && exception_hold_ok
        && control_status_ok
        && rejection_ok
    {
        AssemblyVerificationStatus::Pass
    } else {
        AssemblyVerificationStatus::Fail
    }
}

fn verify_runtime_clear_stop_rereads(
    rereads: &a2ex_live_harness::A2exCanonicalRereads,
    observed_at: &str,
    diagnostics: &mut Vec<AssemblyFailureDiagnostic>,
) -> AssemblyVerificationStatus {
    diagnostics.extend(assembly_diagnostics_from_rereads(
        "runtime_clear_stop_verification_failed",
        observed_at,
        rereads,
    ));

    let operator_hold_cleared = rereads
        .operator_report
        .as_ref()
        .map(|report| report.hold_reason.is_none())
        .unwrap_or(false);
    let report_window_hold_cleared = rereads
        .report_window
        .as_ref()
        .map(|window| window.current_operator_report.hold_reason.is_none())
        .unwrap_or(false);
    let exception_hold_cleared = rereads
        .exception_rollup
        .as_ref()
        .map(|rollup| rollup.active_hold.is_none())
        .unwrap_or(false);
    let control_status_ok = rereads
        .runtime_control_status
        .as_ref()
        .map(|status| status.control_mode == "active" && status.autonomy_eligibility == "eligible")
        .unwrap_or(false);
    let rejection_preserved = rereads
        .runtime_control_failures
        .as_ref()
        .and_then(|failures| failures.last_rejection.as_ref())
        .map(|rejection| rejection.code == "runtime_stopped")
        .unwrap_or(false);

    if !operator_hold_cleared {
        diagnostics.push(stage_failure(
            "runtime_clear_stop_verification_failed",
            "operator-report reread did not clear the active runtime-control hold after runtime.clear_stop",
            vec!["operator-report".to_owned()],
            observed_at,
        ));
    }
    if !report_window_hold_cleared {
        diagnostics.push(stage_failure(
            "runtime_clear_stop_verification_failed",
            "report-window reread did not clear the active runtime-control hold after runtime.clear_stop",
            vec!["report-window".to_owned()],
            observed_at,
        ));
    }
    if !exception_hold_cleared {
        diagnostics.push(stage_failure(
            "runtime_clear_stop_verification_failed",
            "exception-rollup reread did not clear active_hold after runtime.clear_stop",
            vec!["exception-rollup".to_owned()],
            observed_at,
        ));
    }
    if !control_status_ok {
        diagnostics.push(stage_failure(
            "runtime_clear_stop_verification_failed",
            "runtime control status reread did not return to control_mode=active and autonomy_eligibility=eligible after runtime.clear_stop",
            vec!["runtime control status".to_owned()],
            observed_at,
        ));
    }
    if !rejection_preserved {
        diagnostics.push(stage_failure(
            "runtime_clear_stop_verification_failed",
            "runtime control failures reread did not preserve last_rejection.code=runtime_stopped after runtime.clear_stop",
            vec!["runtime control failures".to_owned()],
            observed_at,
        ));
    }

    if operator_hold_cleared
        && report_window_hold_cleared
        && exception_hold_cleared
        && control_status_ok
        && rejection_preserved
    {
        AssemblyVerificationStatus::Pass
    } else {
        AssemblyVerificationStatus::Fail
    }
}

fn stage_failure(
    code: &str,
    summary: &str,
    evidence_refs: Vec<String>,
    observed_at: &str,
) -> AssemblyFailureDiagnostic {
    AssemblyFailureDiagnostic {
        code: code.to_owned(),
        summary: summary.to_owned(),
        evidence_refs,
        observed_at: observed_at.to_owned(),
    }
}

async fn apply_runtime_stop(
    state_db_path: &Path,
    observed_at: &str,
) -> Result<PersistedRuntimeControl, String> {
    let repository = StateRepository::open(state_db_path)
        .await
        .map_err(|error| {
            format!("state repository open failed for runtime.stop verification: {error}")
        })?;
    let mut record = repository
        .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
        .await
        .map_err(|error| {
            format!("runtime control load failed for runtime.stop verification: {error}")
        })?
        .unwrap_or_else(default_runtime_control_record);
    record.control_mode = "stopped".to_owned();
    record.transition_reason = "s04_runtime_stop_verification".to_owned();
    record.transition_source = "a2ex-live-harness.s04".to_owned();
    record.transitioned_at = observed_at.to_owned();
    if record.last_rejection_code.as_deref() != Some("runtime_stopped") {
        record.last_rejection_code = Some("runtime_stopped".to_owned());
        record.last_rejection_message = Some(
            "runtime is stopped; clear_stop before autonomous operation can resume".to_owned(),
        );
        record.last_rejection_operation = Some("autonomous_runtime".to_owned());
        record.last_rejection_at = Some(observed_at.to_owned());
    }
    record.updated_at = observed_at.to_owned();
    repository
        .persist_runtime_control(&record)
        .await
        .map_err(|error| format!("runtime.stop verification persist failed: {error}"))?;
    Ok(record)
}

async fn apply_runtime_clear_stop(
    state_db_path: &Path,
    observed_at: &str,
) -> Result<PersistedRuntimeControl, String> {
    let repository = StateRepository::open(state_db_path)
        .await
        .map_err(|error| {
            format!("state repository open failed for runtime.clear_stop verification: {error}")
        })?;
    let mut record = repository
        .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
        .await
        .map_err(|error| {
            format!("runtime control load failed for runtime.clear_stop verification: {error}")
        })?
        .unwrap_or_else(default_runtime_control_record);
    record.control_mode = "active".to_owned();
    record.transition_reason = "s04_runtime_clear_stop_verification".to_owned();
    record.transition_source = "a2ex-live-harness.s04".to_owned();
    record.transitioned_at = observed_at.to_owned();
    record.last_cleared_at = Some(observed_at.to_owned());
    record.last_cleared_reason = Some("s04_runtime_clear_stop_verification".to_owned());
    record.last_cleared_source = Some("a2ex-live-harness.s04".to_owned());
    record.updated_at = observed_at.to_owned();
    repository
        .persist_runtime_control(&record)
        .await
        .map_err(|error| format!("runtime.clear_stop verification persist failed: {error}"))?;
    Ok(record)
}

fn default_runtime_control_record() -> PersistedRuntimeControl {
    PersistedRuntimeControl {
        scope_key: AUTONOMOUS_RUNTIME_CONTROL_SCOPE.to_owned(),
        control_mode: "active".to_owned(),
        transition_reason: "default_active_state".to_owned(),
        transition_source: "a2ex-live-harness.s04".to_owned(),
        transitioned_at: now_unix_timestamp(),
        last_cleared_at: None,
        last_cleared_reason: None,
        last_cleared_source: None,
        last_rejection_code: None,
        last_rejection_message: None,
        last_rejection_operation: None,
        last_rejection_at: None,
        updated_at: now_unix_timestamp(),
    }
}

fn persist_evidence_artifacts(
    run_dir: &Path,
    run_state: &mut HarnessRunState,
    openclaw_action_summary: Option<&OpenClawActionSummary>,
) -> std::io::Result<()> {
    run_state.evidence_bundle.pinned_runtime_metadata = run_state.runtime_metadata.clone();
    run_state.refresh_canonical_rereads();
    let openclaw_action_summary = openclaw_action_summary.cloned().unwrap_or(OpenClawActionSummary {
        run_id: run_state.run_id.clone(),
        install_url: run_state.canonical.install_url.clone(),
        request_source: "openclaw-request.json".to_owned(),
        request_path: run_state
            .launch
            .openclaw_request_path
            .clone()
            .unwrap_or_else(|| "openclaw-request.json".to_owned()),
        guidance_path: run_state
            .launch
            .openclaw_guidance_path
            .clone()
            .unwrap_or_else(|| "openclaw-guidance.md".to_owned()),
        stdout_path: run_state
            .launch
            .openclaw_stdout_path
            .clone()
            .unwrap_or_else(|| "openclaw.stdout.log".to_owned()),
        stderr_path: run_state
            .launch
            .openclaw_stderr_path
            .clone()
            .unwrap_or_else(|| "openclaw.stderr.log".to_owned()),
        launch_mode: run_state
            .launch
            .openclaw_mode
            .clone()
            .unwrap_or_else(|| "not_started".to_owned()),
        runtime_command_ref: run_state.runtime_metadata.openclaw.runtime_command.clone(),
        image_ref: run_state.runtime_metadata.openclaw.image_ref.clone(),
        overall_status: "not_started".to_owned(),
        stop_reason: Some("openclaw_not_launched".to_owned()),
        stdout_line_count: 0,
        stderr_line_count: 0,
        typed_phase_summary: Vec::new(),
        summary:
            "OpenClaw action summary unavailable because launch artifacts were never created; bundle remains typed and reconnect-safe."
                .to_owned(),
    });
    run_state.evidence_bundle.bundle_ref = S03_EVIDENCE_BUNDLE_REF.to_owned();
    run_state.evidence_bundle.summary_ref = S03_EVIDENCE_SUMMARY_REF.to_owned();
    run_state.evidence_bundle.openclaw_action_summary_ref =
        S03_OPENCLAW_ACTION_SUMMARY_REF.to_owned();
    run_state.evidence_bundle.assembly_summary_ref = ASSEMBLY_SUMMARY_FILE_NAME.to_owned();
    run_state.evidence_bundle.report_command_ref = S03_REPORT_COMMAND_REF.to_owned();
    run_state.assembly_summary.summary_ref = ASSEMBLY_SUMMARY_FILE_NAME.to_owned();
    run_state.assembly_summary.report_command_ref = S04_REPORT_COMMAND_REF.to_owned();
    run_state.sync_assembly_summary_refs();
    run_state.recompute_final_classification(true);
    if !run_dir.join(S03_OPENCLAW_ACTION_SUMMARY_REF).is_file() {
        fs::write(
            run_dir.join(S03_OPENCLAW_ACTION_SUMMARY_REF),
            serde_json::to_vec_pretty(&openclaw_action_summary)
                .expect("fallback OpenClaw action summary serializes"),
        )?;
    }
    let bundle =
        a2ex_live_harness::EvidenceBundle::from_run_state(run_state, openclaw_action_summary);
    bundle.persist_json(&run_dir.join(S03_EVIDENCE_BUNDLE_REF))?;
    fs::write(
        run_dir.join(S03_EVIDENCE_SUMMARY_REF),
        render_evidence_summary_markdown(&bundle),
    )?;
    let assembly_summary = AssemblySummary::from_run_state(run_state);
    assembly_summary.persist_json(&run_dir.join(ASSEMBLY_SUMMARY_FILE_NAME))?;
    Ok(())
}

async fn execute_live_route_command(
    harness: &HarnessEnv,
    run_dir: &Path,
) -> std::io::Result<LiveRouteExecutionRecord> {
    let Some(command) = harness.live_route_command.clone() else {
        return Ok(LiveRouteExecutionRecord::default());
    };

    fs::create_dir_all(run_dir)?;
    let stdout_path = run_dir.join(LIVE_ROUTE_STDOUT_REF);
    let stderr_path = run_dir.join(LIVE_ROUTE_STDERR_REF);
    let stdout = File::create(&stdout_path)?;
    let stderr = File::create(&stderr_path)?;
    let started_at = now_unix_timestamp();

    let mut child = match Command::new("/bin/sh")
        .arg("-lc")
        .arg(&command)
        .current_dir(&harness.repo_root)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Ok(LiveRouteExecutionRecord {
                command: Some(command),
                stdout_ref: Some(LIVE_ROUTE_STDOUT_REF.to_owned()),
                stderr_ref: Some(LIVE_ROUTE_STDERR_REF.to_owned()),
                status_code: None,
                success: false,
                attempted_at: Some(started_at),
                finished_at: Some(now_unix_timestamp()),
                spawn_error: Some(error.to_string()),
            });
        }
    };

    let status = child.wait().await?;
    Ok(LiveRouteExecutionRecord {
        command: Some(command),
        stdout_ref: Some(LIVE_ROUTE_STDOUT_REF.to_owned()),
        stderr_ref: Some(LIVE_ROUTE_STDERR_REF.to_owned()),
        status_code: status.code(),
        success: status.success(),
        attempted_at: Some(started_at),
        finished_at: Some(now_unix_timestamp()),
        spawn_error: None,
    })
}

fn advance_phase_history(run_state: &mut HarnessRunState) {
    if run_state.install_id.is_some()
        && !run_state
            .phase_history
            .iter()
            .any(|entry| entry.phase == HarnessPhase::InstallBootstrapPending)
    {
        run_state.transition(
            HarnessPhase::InstallBootstrapPending,
            "canonical reread observed install_id after onboarding.bootstrap_install",
        );
    }
    if run_state.proposal_id.is_some() {
        run_state.transition(
            HarnessPhase::ProposalGenerationPending,
            "canonical reread observed proposal_id after skills.generate_proposal_packet",
        );
        run_state.transition(
            HarnessPhase::RouteReadinessPending,
            "canonical reread observed proposal correlation for route-readiness inspection",
        );
    }
    if run_state.selection_id.is_some() {
        run_state.transition(
            HarnessPhase::ApprovalPending,
            "canonical reread observed selection_id before or at strategy_selection.approve",
        );
    }
}

struct HarnessEnv {
    repo_root: PathBuf,
    workspace_root: PathBuf,
    state_db_path: PathBuf,
    install_url: String,
    goal: String,
    launch_mode: OpenClawLaunchMode,
    run_id_override: Option<String>,
    live_route_command: Option<String>,
    destination_receipt_ref: Option<String>,
    destination_tx_hash: Option<String>,
    preflight_config: PreflightConfig,
}

impl HarnessEnv {
    fn from_env() -> Result<Self, String> {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .map_err(io_err)?;
        let install_url = required_env("A2EX_OPENCLAW_INSTALL_URL")?;
        let goal = required_env("A2EX_OPENCLAW_GOAL")?;
        let workspace_root = env::var("A2EX_OPENCLAW_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root.join(".a2ex-openclaw-harness"));
        let state_db_path = env::var("A2EX_OPENCLAW_STATE_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root.join(".a2ex-daemon/state.db"));
        let launch_mode = match env::var("A2EX_OPENCLAW_ATTACH_ONLY") {
            Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => {
                OpenClawLaunchMode::Attach
            }
            _ => OpenClawLaunchMode::Spawn,
        };
        let preflight_config = PreflightConfig {
            openclaw_runtime_command: env::var("A2EX_OPENCLAW_RUNTIME_COMMAND")
                .unwrap_or_else(|_| "openclaw".to_owned()),
            openclaw_image_ref: env::var("A2EX_OPENCLAW_IMAGE_REF").ok(),
            install_url: Some(install_url.clone()),
            waiaas_base_url: env::var("A2EX_WAIAAS_BASE_URL").ok(),
            waiaas_session_id: env::var("A2EX_WAIAAS_SESSION_ID").ok(),
            waiaas_policy_id: env::var("A2EX_WAIAAS_POLICY_ID").ok(),
            prerequisite_names: vec![
                "A2EX_OPENCLAW_INSTALL_URL".to_owned(),
                "A2EX_OPENCLAW_GOAL".to_owned(),
                "A2EX_OPENCLAW_STATE_DB_PATH".to_owned(),
            ],
            required_env_keys: vec![
                "A2EX_OPENCLAW_INSTALL_URL".to_owned(),
                "A2EX_OPENCLAW_GOAL".to_owned(),
                "A2EX_OPENCLAW_STATE_DB_PATH".to_owned(),
                "A2EX_WAIAAS_BASE_URL".to_owned(),
                "A2EX_WAIAAS_SESSION_ID".to_owned(),
                "A2EX_WAIAAS_POLICY_ID".to_owned(),
            ],
            openclaw_version: env::var("A2EX_OPENCLAW_VERSION").ok(),
            waiaas_version: env::var("A2EX_WAIAAS_VERSION").ok(),
        };
        Ok(Self {
            repo_root,
            workspace_root,
            state_db_path,
            install_url,
            goal,
            launch_mode,
            run_id_override: env::var("A2EX_OPENCLAW_RUN_ID_OVERRIDE").ok(),
            live_route_command: env::var("A2EX_S02_LIVE_ROUTE_COMMAND").ok(),
            destination_receipt_ref: env::var("A2EX_S02_DESTINATION_RECEIPT_REF").ok(),
            destination_tx_hash: env::var("A2EX_S02_DESTINATION_TX_HASH").ok(),
            preflight_config,
        })
    }

    fn run_state_path(&self, run_id: &str) -> PathBuf {
        self.workspace_root
            .join("runs")
            .join(run_id)
            .join("run-state.json")
    }

    fn waiaas_authority_request(&self) -> Option<WaiaasAuthorityRequest> {
        Some(WaiaasAuthorityRequest {
            base_url: self.preflight_config.waiaas_base_url.clone()?,
            session_id: self.preflight_config.waiaas_session_id.clone()?,
            policy_id: self.preflight_config.waiaas_policy_id.clone()?,
        })
    }

    fn destination_receipt(&self) -> Option<DestinationChainReceipt> {
        Some(DestinationChainReceipt {
            receipt_ref: self.destination_receipt_ref.clone()?,
            destination_chain: "Base".to_owned(),
            asset: "USDC".to_owned(),
            tx_hash: self.destination_tx_hash.clone(),
            observed_at: now_unix_timestamp(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LiveRouteEvidence {
    route_id: String,
    venue: String,
    source_chain: String,
    destination_chain: String,
    asset: String,
    risk_envelope: String,
    run_id: String,
    install_id: Option<String>,
    proposal_id: Option<String>,
    selection_id: Option<String>,
    wallet_boundary: String,
    authority_decision: AuthorityDecision,
    authority_reason_code: String,
    authority_evidence_ref: Option<String>,
    attempt_phase: LiveAttemptPhase,
    attempt_decision: Verdict,
    reason_code: String,
    decisive_signal: String,
    destination_chain_receipt_ref: Option<String>,
    destination_chain_receipt: Option<DestinationChainReceipt>,
    execution: LiveRouteExecutionRecord,
    summary: String,
}

impl LiveRouteEvidence {
    fn from_run_state(run_state: &HarnessRunState) -> Self {
        Self {
            route_id: run_state.live_route.route_id.clone(),
            venue: run_state.live_route.venue.clone(),
            source_chain: run_state.live_route.source_chain.clone(),
            destination_chain: run_state.live_route.destination_chain.clone(),
            asset: run_state.live_route.asset.clone(),
            risk_envelope: run_state.live_route.risk_envelope.max_notional.clone(),
            run_id: run_state.run_id.clone(),
            install_id: run_state.install_id.clone(),
            proposal_id: run_state.proposal_id.clone(),
            selection_id: run_state.selection_id.clone(),
            wallet_boundary: run_state.waiaas_authority.wallet_boundary.clone(),
            authority_decision: run_state.waiaas_authority.authority_decision,
            authority_reason_code: run_state.waiaas_authority.reason_code.clone(),
            authority_evidence_ref: run_state.waiaas_authority.evidence_ref.clone(),
            attempt_phase: run_state.live_attempt.attempt_phase,
            attempt_decision: run_state.final_classification.verdict,
            reason_code: run_state.final_classification.reason_code.clone(),
            decisive_signal: run_state
                .live_route
                .success_criteria
                .decisive_signal
                .clone(),
            destination_chain_receipt_ref: None,
            destination_chain_receipt: None,
            execution: LiveRouteExecutionRecord::default(),
            summary: run_state.final_classification.summary.clone(),
        }
    }

    fn persist(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(self).expect("live route evidence serializes"),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct LiveRouteExecutionRecord {
    command: Option<String>,
    stdout_ref: Option<String>,
    stderr_ref: Option<String>,
    status_code: Option<i32>,
    success: bool,
    attempted_at: Option<String>,
    finished_at: Option<String>,
    spawn_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DestinationChainReceipt {
    receipt_ref: String,
    destination_chain: String,
    asset: String,
    tx_hash: Option<String>,
    observed_at: String,
}

fn required_env(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("required environment variable {key} is not set"))
}

fn io_err(error: std::io::Error) -> String {
    error.to_string()
}

fn print_summary(run_state: &HarnessRunState, run_state_path: &Path) {
    println!("run_id={}", run_state.run_id);
    println!("run_state={}", run_state_path.display());
    println!("pre_trade_outcome={:?}", run_state.pre_trade_outcome);
    println!("last_phase={:?}", run_state.last_phase);
    println!(
        "live_attempt_phase={:?}",
        run_state.live_attempt.attempt_phase
    );
    println!("final_verdict={:?}", run_state.final_classification.verdict);
    println!(
        "final_reason_code={}",
        run_state.final_classification.reason_code
    );
    if let Some(install_id) = &run_state.install_id {
        println!("install_id={install_id}");
    }
    if let Some(proposal_id) = &run_state.proposal_id {
        println!("proposal_id={proposal_id}");
    }
    if let Some(selection_id) = &run_state.selection_id {
        println!("selection_id={selection_id}");
    }
    if let Some(state_db_path) = &run_state.canonical.state_db_path {
        println!("state_db_path={state_db_path}");
    }
    println!(
        "waiaas_authority_decision={:?}",
        run_state.waiaas_authority.authority_decision
    );
    println!(
        "waiaas_reason_code={}",
        run_state.waiaas_authority.reason_code
    );
    if let Some(authority_ref) = run_state.waiaas_authority.evidence_ref.as_deref() {
        println!("waiaas_evidence_ref={authority_ref}");
    }
    if let Some(live_route_ref) = run_state
        .evidence_refs
        .live_route_evidence
        .evidence_ref
        .as_deref()
    {
        println!("live_route_evidence_ref={live_route_ref}");
    }
    println!(
        "evidence_bundle_ref={}",
        run_state.evidence_bundle.bundle_ref
    );
    println!(
        "evidence_summary_ref={}",
        run_state.evidence_bundle.summary_ref
    );
    println!(
        "openclaw_action_summary_ref={}",
        run_state.evidence_bundle.openclaw_action_summary_ref
    );
    println!(
        "assembly_summary_ref={}",
        run_state.assembly_summary.summary_ref
    );
    println!(
        "report_command_ref={}",
        run_state.evidence_bundle.report_command_ref
    );
    println!("assembly_phase={:?}", run_state.assembly_phase);
    println!(
        "decisive_snapshot_verdict={:?}",
        run_state.decisive_snapshot.verdict
    );
    println!(
        "post_control_report_status={:?}",
        run_state.post_control_verification.report_status
    );
    println!(
        "post_control_runtime_stop_status={:?}",
        run_state.post_control_verification.runtime_stop_status
    );
    println!(
        "post_control_runtime_clear_stop_status={:?}",
        run_state
            .post_control_verification
            .runtime_clear_stop_status
    );
    println!(
        "canonical_rereads.operator_report={}",
        run_state.canonical_rereads.operator_report.evidence_ref
    );
    println!(
        "canonical_rereads.report_window={}",
        run_state.canonical_rereads.report_window.evidence_ref
    );
    println!(
        "canonical_rereads.exception_rollup={}",
        run_state.canonical_rereads.exception_rollup.evidence_ref
    );
    println!(
        "canonical_rereads.runtime_control_status={}",
        run_state
            .canonical_rereads
            .runtime_control_status
            .evidence_ref
    );
    println!(
        "canonical_rereads.runtime_control_failures={}",
        run_state
            .canonical_rereads
            .runtime_control_failures
            .evidence_ref
    );
    if !run_state.canonical_reread_collection.diagnostics.is_empty() {
        for diagnostic in &run_state.canonical_reread_collection.diagnostics {
            println!(
                "canonical_reread_diagnostic={}::{}",
                diagnostic.code, diagnostic.summary
            );
        }
    }
    if !run_state
        .final_classification
        .mismatch_diagnostics
        .is_empty()
    {
        for diagnostic in &run_state.final_classification.mismatch_diagnostics {
            println!(
                "verdict_mismatch_diagnostic={}::{}",
                diagnostic.code, diagnostic.summary
            );
        }
    }
    println!("reconnect_requires=onboarding.bootstrap_install");
    for guidance in &run_state.reconnect.guidance {
        println!("reconnect_guidance={guidance}");
    }
    if let Some(openclaw_request) = run_state.launch.openclaw_request_path.as_deref() {
        println!("openclaw_request={openclaw_request}");
    }
    if let Some(openclaw_guidance) = run_state.launch.openclaw_guidance_path.as_deref() {
        println!("openclaw_guidance={openclaw_guidance}");
    }
}

fn now_unix_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}
