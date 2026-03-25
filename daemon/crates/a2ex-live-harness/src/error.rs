use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    OpenclawUnavailable,
    A2exMcpSpawnFailed,
    InstallUrlUnreachable,
    WaiaasUnhealthy,
    WaiaasSessionMissing,
    WaiaasAuthorityBlocked,
    WaiaasAuthorityHold,
    WaiaasAuthorityFailed,
    A2exCanonicalRereadFailed,
    A2exCanonicalRereadMissing,
    VerdictClassificationFailed,
    CanonicalTruthMismatch,
    PrerequisiteMissing,
}

impl ErrorClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenclawUnavailable => "openclaw_unavailable",
            Self::A2exMcpSpawnFailed => "a2ex_mcp_spawn_failed",
            Self::InstallUrlUnreachable => "install_url_unreachable",
            Self::WaiaasUnhealthy => "waiaas_unhealthy",
            Self::WaiaasSessionMissing => "waiaas_session_missing",
            Self::WaiaasAuthorityBlocked => "waiaas_authority_blocked",
            Self::WaiaasAuthorityHold => "waiaas_authority_hold",
            Self::WaiaasAuthorityFailed => "waiaas_authority_failed",
            Self::A2exCanonicalRereadFailed => "a2ex_canonical_reread_failed",
            Self::A2exCanonicalRereadMissing => "a2ex_canonical_reread_missing",
            Self::VerdictClassificationFailed => "verdict_classification_failed",
            Self::CanonicalTruthMismatch => "canonical_truth_mismatch",
            Self::PrerequisiteMissing => "prerequisite_missing",
        }
    }
}

impl fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Missing,
    Timeout,
    Connect,
    HttpStatus,
    Spawn,
    InvalidResponse,
    Blocked,
    Hold,
    ExecutionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessIssue {
    pub class: ErrorClass,
    pub failure_kind: FailureKind,
    pub subject: String,
    pub message: String,
    pub detail: Option<String>,
}

impl HarnessIssue {
    pub fn new(
        class: ErrorClass,
        failure_kind: FailureKind,
        subject: impl Into<String>,
        message: impl Into<String>,
        detail: Option<String>,
    ) -> Self {
        Self {
            class,
            failure_kind,
            subject: subject.into(),
            message: message.into(),
            detail,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("preflight failed")]
    PreflightFailed(Vec<HarnessIssue>),
}

pub type HarnessResult<T> = Result<T, HarnessError>;
