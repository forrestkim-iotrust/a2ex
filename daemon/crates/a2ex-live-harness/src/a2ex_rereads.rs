use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use a2ex_onboarding::{
    InspectStrategyReportWindowRequest, InspectStrategyRuntimeRequest, StrategyExceptionRollup,
    StrategyOperatorReport, StrategyReportWindow, inspect_strategy_exception_rollup,
    inspect_strategy_operator_report, inspect_strategy_report_window,
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::evidence_bundle::CanonicalRereadRefs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct A2exRereadDiagnostic {
    pub code: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeControlRejectionSnapshot {
    pub code: String,
    pub message: String,
    pub attempted_operation: String,
    pub rejected_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeControlStatusSnapshot {
    pub install_id: String,
    pub scope_key: String,
    pub control_mode: String,
    pub autonomy_eligibility: String,
    pub transition_reason: String,
    pub transition_source: String,
    pub transitioned_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cleared_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cleared_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cleared_source: Option<String>,
    pub updated_at: String,
    pub status_uri: String,
    pub failures_uri: String,
    pub derived_from_default_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeControlFailuresSnapshot {
    pub install_id: String,
    pub scope_key: String,
    pub control_mode: String,
    pub autonomy_eligibility: String,
    pub transition_reason: String,
    pub transition_source: String,
    pub transitioned_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cleared_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cleared_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cleared_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection: Option<RuntimeControlRejectionSnapshot>,
    pub updated_at: String,
    pub status_uri: String,
    pub failures_uri: String,
    pub derived_from_default_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct A2exCanonicalRereads {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_report: Option<StrategyOperatorReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_window: Option<StrategyReportWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_rollup: Option<StrategyExceptionRollup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_control_status: Option<RuntimeControlStatusSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_control_failures: Option<RuntimeControlFailuresSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<A2exRereadDiagnostic>,
}

impl A2exCanonicalRereads {
    pub fn collected_successfully(&self) -> bool {
        self.operator_report.is_some()
            && self.report_window.is_some()
            && self.exception_rollup.is_some()
            && self.runtime_control_status.is_some()
            && self.runtime_control_failures.is_some()
            && self.diagnostics.is_empty()
    }
}

pub async fn collect_canonical_rereads(
    state_db_path: &Path,
    install_id: Option<&str>,
    proposal_id: Option<&str>,
    selection_id: Option<&str>,
    refs: &CanonicalRereadRefs,
) -> A2exCanonicalRereads {
    let mut rereads = A2exCanonicalRereads {
        collected_at: Some(now_timestamp()),
        ..A2exCanonicalRereads::default()
    };

    match (install_id, proposal_id, selection_id) {
        (Some(install_id), Some(proposal_id), Some(selection_id)) => {
            let runtime_request = InspectStrategyRuntimeRequest {
                state_db_path: state_db_path.to_path_buf(),
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
            };
            let report_window_request = InspectStrategyReportWindowRequest {
                state_db_path: state_db_path.to_path_buf(),
                install_id: install_id.to_owned(),
                proposal_id: proposal_id.to_owned(),
                selection_id: selection_id.to_owned(),
                cursor: "bootstrap".to_owned(),
                window_limit: 25,
            };

            match inspect_strategy_operator_report(runtime_request.clone()).await {
                Ok(report) => rereads.operator_report = Some(report),
                Err(error) => rereads.diagnostics.push(A2exRereadDiagnostic {
                    code: "operator_report_reread_failed".to_owned(),
                    summary: error.to_string(),
                    evidence_refs: vec![refs.operator_report.evidence_ref.clone()],
                }),
            }
            match inspect_strategy_report_window(report_window_request).await {
                Ok(report_window) => rereads.report_window = Some(report_window),
                Err(error) => rereads.diagnostics.push(A2exRereadDiagnostic {
                    code: "report_window_reread_failed".to_owned(),
                    summary: error.to_string(),
                    evidence_refs: vec![refs.report_window.evidence_ref.clone()],
                }),
            }
            match inspect_strategy_exception_rollup(runtime_request).await {
                Ok(exception_rollup) => rereads.exception_rollup = Some(exception_rollup),
                Err(error) => rereads.diagnostics.push(A2exRereadDiagnostic {
                    code: "exception_rollup_reread_failed".to_owned(),
                    summary: error.to_string(),
                    evidence_refs: vec![refs.exception_rollup.evidence_ref.clone()],
                }),
            }
        }
        _ => rereads.diagnostics.push(A2exRereadDiagnostic {
            code: "selection_identity_missing_for_strategy_rereads".to_owned(),
            summary: "install_id, proposal_id, and selection_id are required before operator-report, report-window, and exception-rollup can be reread".to_owned(),
            evidence_refs: vec![
                refs.operator_report.evidence_ref.clone(),
                refs.report_window.evidence_ref.clone(),
                refs.exception_rollup.evidence_ref.clone(),
            ],
        }),
    }

    if let Some(install_id) = install_id {
        match reread_runtime_control(state_db_path, install_id, refs) {
            Ok((status, failures)) => {
                rereads.runtime_control_status = Some(status);
                rereads.runtime_control_failures = Some(failures);
            }
            Err(error) => rereads.diagnostics.push(A2exRereadDiagnostic {
                code: "runtime_control_reread_failed".to_owned(),
                summary: error,
                evidence_refs: vec![
                    refs.runtime_control_status.evidence_ref.clone(),
                    refs.runtime_control_failures.evidence_ref.clone(),
                ],
            }),
        }
    } else {
        rereads.diagnostics.push(A2exRereadDiagnostic {
            code: "install_identity_missing_for_runtime_control_rereads".to_owned(),
            summary:
                "install_id is required before runtime control status and failures can be reread"
                    .to_owned(),
            evidence_refs: vec![
                refs.runtime_control_status.evidence_ref.clone(),
                refs.runtime_control_failures.evidence_ref.clone(),
            ],
        });
    }

    rereads
}

fn reread_runtime_control(
    state_db_path: &Path,
    install_id: &str,
    refs: &CanonicalRereadRefs,
) -> Result<(RuntimeControlStatusSnapshot, RuntimeControlFailuresSnapshot), String> {
    let connection = Connection::open(state_db_path)
        .map_err(|error| format!("state.db open failed for runtime control reread: {error}"))?;
    let row = connection
        .query_row(
            "SELECT scope_key, control_mode, transition_reason, transition_source, transitioned_at,
                    last_cleared_at, last_cleared_reason, last_cleared_source,
                    last_rejection_code, last_rejection_message, last_rejection_operation,
                    last_rejection_at, updated_at
             FROM runtime_control WHERE scope_key = ?1",
            ["autonomous_runtime"],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("runtime_control reread failed: {error}"))?;

    let status_uri = refs.runtime_control_status.evidence_ref.clone();
    let failures_uri = refs.runtime_control_failures.evidence_ref.clone();

    let (status, failures) = match row {
        Some((
            scope_key,
            control_mode,
            transition_reason,
            transition_source,
            transitioned_at,
            last_cleared_at,
            last_cleared_reason,
            last_cleared_source,
            last_rejection_code,
            last_rejection_message,
            last_rejection_operation,
            last_rejection_at,
            updated_at,
        )) => {
            let autonomy_eligibility = if control_mode == "active" {
                "eligible"
            } else {
                "blocked"
            }
            .to_owned();
            let last_rejection = match (
                last_rejection_code,
                last_rejection_message,
                last_rejection_operation,
                last_rejection_at,
            ) {
                (Some(code), Some(message), Some(attempted_operation), Some(rejected_at)) => {
                    Some(RuntimeControlRejectionSnapshot {
                        code,
                        message,
                        attempted_operation,
                        rejected_at,
                    })
                }
                _ => None,
            };
            (
                RuntimeControlStatusSnapshot {
                    install_id: install_id.to_owned(),
                    scope_key: scope_key.clone(),
                    control_mode: control_mode.clone(),
                    autonomy_eligibility: autonomy_eligibility.clone(),
                    transition_reason: transition_reason.clone(),
                    transition_source: transition_source.clone(),
                    transitioned_at: transitioned_at.clone(),
                    last_cleared_at: last_cleared_at.clone(),
                    last_cleared_reason: last_cleared_reason.clone(),
                    last_cleared_source: last_cleared_source.clone(),
                    updated_at: updated_at.clone(),
                    status_uri: status_uri.clone(),
                    failures_uri: failures_uri.clone(),
                    derived_from_default_state: false,
                },
                RuntimeControlFailuresSnapshot {
                    install_id: install_id.to_owned(),
                    scope_key,
                    control_mode,
                    autonomy_eligibility,
                    transition_reason,
                    transition_source,
                    transitioned_at,
                    last_cleared_at,
                    last_cleared_reason,
                    last_cleared_source,
                    last_rejection,
                    updated_at,
                    status_uri,
                    failures_uri,
                    derived_from_default_state: false,
                },
            )
        }
        None => {
            let updated_at = now_timestamp();
            (
                RuntimeControlStatusSnapshot {
                    install_id: install_id.to_owned(),
                    scope_key: "autonomous_runtime".to_owned(),
                    control_mode: "active".to_owned(),
                    autonomy_eligibility: "eligible".to_owned(),
                    transition_reason: "default_active_state".to_owned(),
                    transition_source: "state_db_default".to_owned(),
                    transitioned_at: updated_at.clone(),
                    last_cleared_at: None,
                    last_cleared_reason: None,
                    last_cleared_source: None,
                    updated_at: updated_at.clone(),
                    status_uri: status_uri.clone(),
                    failures_uri: failures_uri.clone(),
                    derived_from_default_state: true,
                },
                RuntimeControlFailuresSnapshot {
                    install_id: install_id.to_owned(),
                    scope_key: "autonomous_runtime".to_owned(),
                    control_mode: "active".to_owned(),
                    autonomy_eligibility: "eligible".to_owned(),
                    transition_reason: "default_active_state".to_owned(),
                    transition_source: "state_db_default".to_owned(),
                    transitioned_at: updated_at.clone(),
                    last_cleared_at: None,
                    last_cleared_reason: None,
                    last_cleared_source: None,
                    last_rejection: None,
                    updated_at,
                    status_uri,
                    failures_uri,
                    derived_from_default_state: true,
                },
            )
        }
    };

    Ok((status, failures))
}

fn now_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}
