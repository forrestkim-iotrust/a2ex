use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{ErrorClass, FailureKind, HarnessIssue};
use crate::runtime_metadata::{RuntimeMetadata, RuntimeMetadataInput};
use crate::waiaas::WaiaasAuthorityOutcome;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightConfig {
    pub openclaw_runtime_command: String,
    pub openclaw_image_ref: Option<String>,
    pub install_url: Option<String>,
    pub waiaas_base_url: Option<String>,
    pub waiaas_session_id: Option<String>,
    pub waiaas_policy_id: Option<String>,
    pub prerequisite_names: Vec<String>,
    pub required_env_keys: Vec<String>,
    pub openclaw_version: Option<String>,
    pub waiaas_version: Option<String>,
}

impl Default for PreflightConfig {
    fn default() -> Self {
        Self {
            openclaw_runtime_command: "openclaw".to_owned(),
            openclaw_image_ref: None,
            install_url: None,
            waiaas_base_url: None,
            waiaas_session_id: None,
            waiaas_policy_id: None,
            prerequisite_names: Vec::new(),
            required_env_keys: Vec::new(),
            openclaw_version: None,
            waiaas_version: None,
        }
    }
}

impl PreflightConfig {
    pub fn runtime_metadata_input(&self) -> RuntimeMetadataInput {
        RuntimeMetadataInput {
            openclaw_runtime_command: self.openclaw_runtime_command.clone(),
            openclaw_version: self.openclaw_version.clone(),
            openclaw_image_ref: self.openclaw_image_ref.clone(),
            waiaas_base_url: self.waiaas_base_url.clone(),
            waiaas_version: self.waiaas_version.clone(),
            env_keys: self.required_env_keys.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightReport {
    pub status: PreflightStatus,
    pub checks: Vec<PreflightCheckResult>,
    pub failures: Vec<HarnessIssue>,
    pub runtime_metadata: RuntimeMetadata,
}

impl PreflightReport {
    pub fn is_ok(&self) -> bool {
        self.status == PreflightStatus::Passed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightCheckResult {
    pub name: String,
    pub ok: bool,
    pub issue: Option<HarnessIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaiaasAuthoritySemantics {
    Pass,
    Blocked,
    Hold,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaiaasAuthorityClassification {
    pub semantics: WaiaasAuthoritySemantics,
    pub reason_code: String,
    pub issue: Option<HarnessIssue>,
}

impl WaiaasAuthorityClassification {
    pub fn from_outcome(outcome: &WaiaasAuthorityOutcome) -> Self {
        let capture = outcome.capture();
        let semantics = match outcome {
            WaiaasAuthorityOutcome::Pass(_) => WaiaasAuthoritySemantics::Pass,
            WaiaasAuthorityOutcome::Blocked(_) => WaiaasAuthoritySemantics::Blocked,
            WaiaasAuthorityOutcome::Hold(_) => WaiaasAuthoritySemantics::Hold,
            WaiaasAuthorityOutcome::Fail(_) => WaiaasAuthoritySemantics::Fail,
        };
        let issue = match semantics {
            WaiaasAuthoritySemantics::Pass => None,
            WaiaasAuthoritySemantics::Blocked => Some(HarnessIssue::new(
                ErrorClass::WaiaasAuthorityBlocked,
                FailureKind::Blocked,
                "waiaas_authority",
                capture.reason_code.clone(),
                Some(capture.evidence_ref.clone()),
            )),
            WaiaasAuthoritySemantics::Hold => Some(HarnessIssue::new(
                ErrorClass::WaiaasAuthorityHold,
                FailureKind::Hold,
                "waiaas_authority",
                capture.reason_code.clone(),
                Some(capture.evidence_ref.clone()),
            )),
            WaiaasAuthoritySemantics::Fail => Some(HarnessIssue::new(
                ErrorClass::WaiaasAuthorityFailed,
                FailureKind::ExecutionFailed,
                "waiaas_authority",
                capture.reason_code.clone(),
                Some(capture.evidence_ref.clone()),
            )),
        };
        Self {
            semantics,
            reason_code: capture.reason_code.clone(),
            issue,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSnapshot {
    pub openclaw_runtime: ProbeOutcome,
    pub openclaw_image: ProbeOutcome,
    pub a2ex_mcp_spawn: ProbeOutcome,
    pub install_url: ProbeOutcome,
    pub waiaas_health: ProbeOutcome,
    pub waiaas_session: ProbeOutcome,
    pub prerequisites: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok,
    Missing { message: String },
    Timeout { message: String },
    Connect { message: String },
    Status { code: u16, message: String },
    Spawn { message: String },
    InvalidResponse { message: String },
}

impl ProbeOutcome {
    fn into_issue(self, class: ErrorClass, subject: impl Into<String>) -> Option<HarnessIssue> {
        let subject = subject.into();
        match self {
            Self::Ok => None,
            Self::Missing { message } => Some(HarnessIssue::new(
                class,
                FailureKind::Missing,
                subject,
                message,
                None,
            )),
            Self::Timeout { message } => Some(HarnessIssue::new(
                class,
                FailureKind::Timeout,
                subject,
                message,
                None,
            )),
            Self::Connect { message } => Some(HarnessIssue::new(
                class,
                FailureKind::Connect,
                subject,
                message,
                None,
            )),
            Self::Status { code, message } => Some(HarnessIssue::new(
                class,
                FailureKind::HttpStatus,
                subject,
                message,
                Some(format!("http_status={code}")),
            )),
            Self::Spawn { message } => Some(HarnessIssue::new(
                class,
                FailureKind::Spawn,
                subject,
                message,
                None,
            )),
            Self::InvalidResponse { message } => Some(HarnessIssue::new(
                class,
                FailureKind::InvalidResponse,
                subject,
                message,
                None,
            )),
        }
    }
}

impl ProbeSnapshot {
    pub fn from_config(config: &PreflightConfig) -> Self {
        Self {
            openclaw_runtime: if config.openclaw_runtime_command.trim().is_empty() {
                ProbeOutcome::Missing {
                    message: "OpenClaw runtime command is not configured".to_owned(),
                }
            } else {
                ProbeOutcome::Ok
            },
            openclaw_image: if config
                .openclaw_image_ref
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                ProbeOutcome::Missing {
                    message: "OpenClaw image reference is not configured".to_owned(),
                }
            } else {
                ProbeOutcome::Ok
            },
            a2ex_mcp_spawn: ProbeOutcome::Ok,
            install_url: if config
                .install_url
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                ProbeOutcome::Missing {
                    message: "install URL is not configured".to_owned(),
                }
            } else {
                ProbeOutcome::Ok
            },
            waiaas_health: if config
                .waiaas_base_url
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                ProbeOutcome::Missing {
                    message: "WAIaaS base URL is not configured".to_owned(),
                }
            } else {
                ProbeOutcome::Ok
            },
            waiaas_session: if config
                .waiaas_session_id
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
                || config
                    .waiaas_policy_id
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                ProbeOutcome::Missing {
                    message: "WAIaaS session/policy discovery is incomplete".to_owned(),
                }
            } else {
                ProbeOutcome::Ok
            },
            prerequisites: config
                .prerequisite_names
                .iter()
                .filter(|name| std::env::var_os(name).is_none())
                .cloned()
                .collect(),
        }
    }
}

pub fn build_report(config: &PreflightConfig, snapshot: ProbeSnapshot) -> PreflightReport {
    let runtime_metadata = config.runtime_metadata_input().capture();
    let mut checks = Vec::new();
    let mut failures = Vec::new();

    push_check(
        &mut checks,
        &mut failures,
        "openclaw_runtime",
        snapshot.openclaw_runtime,
        ErrorClass::OpenclawUnavailable,
        "openclaw_runtime",
    );
    push_check(
        &mut checks,
        &mut failures,
        "openclaw_image",
        snapshot.openclaw_image,
        ErrorClass::OpenclawUnavailable,
        "openclaw_image",
    );
    push_check(
        &mut checks,
        &mut failures,
        "a2ex_mcp_spawn",
        snapshot.a2ex_mcp_spawn,
        ErrorClass::A2exMcpSpawnFailed,
        "a2ex-mcp",
    );
    push_check(
        &mut checks,
        &mut failures,
        "install_url",
        snapshot.install_url,
        ErrorClass::InstallUrlUnreachable,
        "install_url",
    );
    push_check(
        &mut checks,
        &mut failures,
        "waiaas_health",
        snapshot.waiaas_health,
        ErrorClass::WaiaasUnhealthy,
        "waiaas_health",
    );
    push_check(
        &mut checks,
        &mut failures,
        "waiaas_session",
        snapshot.waiaas_session,
        ErrorClass::WaiaasSessionMissing,
        "waiaas_session",
    );

    let prerequisite_issue = if snapshot.prerequisites.is_empty() {
        None
    } else {
        Some(HarnessIssue::new(
            ErrorClass::PrerequisiteMissing,
            FailureKind::Missing,
            "prerequisites",
            format!(
                "missing named prerequisites: {}",
                snapshot.prerequisites.join(", ")
            ),
            Some(snapshot.prerequisites.join(",")),
        ))
    };
    if let Some(issue) = prerequisite_issue.clone() {
        failures.push(issue);
    }
    checks.push(PreflightCheckResult {
        name: "prerequisites".to_owned(),
        ok: prerequisite_issue.is_none(),
        issue: prerequisite_issue,
    });

    PreflightReport {
        status: if failures.is_empty() {
            PreflightStatus::Passed
        } else {
            PreflightStatus::Failed
        },
        checks,
        failures,
        runtime_metadata,
    }
}

fn push_check(
    checks: &mut Vec<PreflightCheckResult>,
    failures: &mut Vec<HarnessIssue>,
    name: &'static str,
    outcome: ProbeOutcome,
    class: ErrorClass,
    subject: &'static str,
) {
    let issue = outcome.into_issue(class, subject);
    if let Some(issue) = issue.clone() {
        failures.push(issue);
    }
    checks.push(PreflightCheckResult {
        name: name.to_owned(),
        ok: issue.is_none(),
        issue,
    });
}

#[async_trait]
pub trait PreflightProbe {
    async fn snapshot(&self, config: &PreflightConfig) -> ProbeSnapshot;
}

#[derive(Debug, Default)]
pub struct LocalPreflightProbe;

#[async_trait]
impl PreflightProbe for LocalPreflightProbe {
    async fn snapshot(&self, config: &PreflightConfig) -> ProbeSnapshot {
        ProbeSnapshot::from_config(config)
    }
}

pub async fn run_preflight(
    config: &PreflightConfig,
    probe: &impl PreflightProbe,
) -> PreflightReport {
    build_report(config, probe.snapshot(config).await)
}
