use std::{io, path::Path, time::Duration};

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::{ErrorClass, FailureKind, HarnessIssue},
    run_state::AuthorityDecision,
};

pub const DEFAULT_WALLET_BOUNDARY: &str = "same WAIaaS-governed wallet boundary";
pub const DEFAULT_AUTHORITY_EVIDENCE_REF: &str = "waiaas-authority.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaiaasAuthorityEvidence {
    pub session_id: Option<String>,
    pub policy_id: Option<String>,
    pub authority_decision: AuthorityDecision,
    pub reason_code: String,
    pub authority_timestamp: Option<String>,
    pub wallet_boundary: String,
}

impl Default for WaiaasAuthorityEvidence {
    fn default() -> Self {
        Self {
            session_id: None,
            policy_id: None,
            authority_decision: AuthorityDecision::Hold,
            reason_code: "awaiting_waiaas_authority_check".to_owned(),
            authority_timestamp: None,
            wallet_boundary: DEFAULT_WALLET_BOUNDARY.to_owned(),
        }
    }
}

impl WaiaasAuthorityEvidence {
    pub fn blocked(
        session_id: Option<String>,
        policy_id: Option<String>,
        reason_code: impl Into<String>,
        authority_timestamp: Option<String>,
    ) -> Self {
        Self {
            session_id,
            policy_id,
            authority_decision: AuthorityDecision::Blocked,
            reason_code: reason_code.into(),
            authority_timestamp,
            wallet_boundary: DEFAULT_WALLET_BOUNDARY.to_owned(),
        }
    }

    pub fn fail(
        session_id: Option<String>,
        policy_id: Option<String>,
        reason_code: impl Into<String>,
        authority_timestamp: Option<String>,
    ) -> Self {
        Self {
            session_id,
            policy_id,
            authority_decision: AuthorityDecision::Fail,
            reason_code: reason_code.into(),
            authority_timestamp,
            wallet_boundary: DEFAULT_WALLET_BOUNDARY.to_owned(),
        }
    }

    pub fn hold(
        session_id: Option<String>,
        policy_id: Option<String>,
        reason_code: impl Into<String>,
        authority_timestamp: Option<String>,
    ) -> Self {
        Self {
            session_id,
            policy_id,
            authority_decision: AuthorityDecision::Hold,
            reason_code: reason_code.into(),
            authority_timestamp,
            wallet_boundary: DEFAULT_WALLET_BOUNDARY.to_owned(),
        }
    }

    pub fn pass(
        session_id: Option<String>,
        policy_id: Option<String>,
        reason_code: impl Into<String>,
        authority_timestamp: Option<String>,
    ) -> Self {
        Self {
            session_id,
            policy_id,
            authority_decision: AuthorityDecision::Pass,
            reason_code: reason_code.into(),
            authority_timestamp,
            wallet_boundary: DEFAULT_WALLET_BOUNDARY.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaiaasAuthorityRequest {
    pub base_url: String,
    pub session_id: String,
    pub policy_id: String,
}

impl WaiaasAuthorityRequest {
    pub fn health_url(&self) -> String {
        format!("{}/health", self.base_url.trim_end_matches('/'))
    }

    pub fn session_url(&self) -> String {
        format!(
            "{}/sessions/{}",
            self.base_url.trim_end_matches('/'),
            self.session_id
        )
    }

    pub fn policy_url(&self) -> String {
        format!(
            "{}/policies/{}",
            self.base_url.trim_end_matches('/'),
            self.policy_id
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaiaasProbeEvidence {
    pub probe: String,
    pub url: String,
    pub outcome: String,
    pub status_code: Option<u16>,
    pub reason_code: String,
    pub observed_at: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaiaasAuthorityCapture {
    pub session_id: Option<String>,
    pub policy_id: Option<String>,
    pub authority_decision: AuthorityDecision,
    pub reason_code: String,
    pub authority_timestamp: Option<String>,
    pub wallet_boundary: String,
    pub evidence_ref: String,
    pub health_url: String,
    pub session_url: String,
    pub policy_url: String,
    pub probes: Vec<WaiaasProbeEvidence>,
}

impl WaiaasAuthorityCapture {
    pub fn from_evidence(
        request: &WaiaasAuthorityRequest,
        evidence: WaiaasAuthorityEvidence,
    ) -> Self {
        Self {
            session_id: evidence.session_id.clone(),
            policy_id: evidence.policy_id.clone(),
            authority_decision: evidence.authority_decision,
            reason_code: evidence.reason_code.clone(),
            authority_timestamp: evidence.authority_timestamp.clone(),
            wallet_boundary: evidence.wallet_boundary,
            evidence_ref: DEFAULT_AUTHORITY_EVIDENCE_REF.to_owned(),
            health_url: request.health_url(),
            session_url: request.session_url(),
            policy_url: request.policy_url(),
            probes: Vec::new(),
        }
    }

    pub fn evidence(&self) -> WaiaasAuthorityEvidence {
        WaiaasAuthorityEvidence {
            session_id: self.session_id.clone(),
            policy_id: self.policy_id.clone(),
            authority_decision: self.authority_decision,
            reason_code: self.reason_code.clone(),
            authority_timestamp: self.authority_timestamp.clone(),
            wallet_boundary: self.wallet_boundary.clone(),
        }
    }

    pub fn persist(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            serde_json::to_vec_pretty(self).expect("waiaas authority capture serializes"),
        )
    }

    pub fn issue(&self) -> Option<HarnessIssue> {
        let (class, failure_kind) = match self.authority_decision {
            AuthorityDecision::Pass => return None,
            AuthorityDecision::Blocked => {
                (ErrorClass::WaiaasAuthorityBlocked, FailureKind::Blocked)
            }
            AuthorityDecision::Hold => (ErrorClass::WaiaasAuthorityHold, FailureKind::Hold),
            AuthorityDecision::Fail => (
                ErrorClass::WaiaasAuthorityFailed,
                FailureKind::ExecutionFailed,
            ),
        };
        Some(HarnessIssue::new(
            class,
            failure_kind,
            "waiaas_authority",
            self.reason_code.clone(),
            Some(self.evidence_ref.clone()),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaiaasAuthorityOutcome {
    Pass(WaiaasAuthorityCapture),
    Blocked(WaiaasAuthorityCapture),
    Hold(WaiaasAuthorityCapture),
    Fail(WaiaasAuthorityCapture),
}

impl WaiaasAuthorityOutcome {
    pub fn pass(capture: WaiaasAuthorityCapture) -> Self {
        Self::Pass(capture)
    }

    pub fn blocked(capture: WaiaasAuthorityCapture) -> Self {
        Self::Blocked(capture)
    }

    pub fn hold(capture: WaiaasAuthorityCapture) -> Self {
        Self::Hold(capture)
    }

    pub fn fail(capture: WaiaasAuthorityCapture) -> Self {
        Self::Fail(capture)
    }

    pub fn capture(&self) -> &WaiaasAuthorityCapture {
        match self {
            Self::Pass(capture)
            | Self::Blocked(capture)
            | Self::Hold(capture)
            | Self::Fail(capture) => capture,
        }
    }

    pub fn authority_decision(&self) -> AuthorityDecision {
        self.capture().authority_decision
    }

    pub fn issue(&self) -> Option<HarnessIssue> {
        self.capture().issue()
    }
}

#[async_trait]
pub trait WaiaasAuthorityAdapter {
    async fn inspect(&self, request: &WaiaasAuthorityRequest) -> WaiaasAuthorityOutcome;
}

#[derive(Debug, Clone)]
pub struct HttpWaiaasAuthorityAdapter {
    client: Client,
}

impl Default for HttpWaiaasAuthorityAdapter {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest client builds"),
        }
    }
}

#[async_trait]
impl WaiaasAuthorityAdapter for HttpWaiaasAuthorityAdapter {
    async fn inspect(&self, request: &WaiaasAuthorityRequest) -> WaiaasAuthorityOutcome {
        let observed_at = now_unix_timestamp();
        let mut probes = Vec::new();

        let health_probe = self
            .probe_health(request.health_url(), observed_at.clone())
            .await;
        probes.push(health_probe.evidence.clone());
        if let Some(outcome) = health_probe.classified_outcome(request, &probes) {
            return outcome;
        }

        let session_probe = self
            .probe_resource(
                "session",
                request.session_url(),
                request.session_id.clone(),
                observed_at.clone(),
            )
            .await;
        probes.push(session_probe.evidence.clone());
        if let Some(outcome) = session_probe.classified_outcome(request, &probes) {
            return outcome;
        }

        let policy_probe = self
            .probe_resource(
                "policy",
                request.policy_url(),
                request.policy_id.clone(),
                observed_at.clone(),
            )
            .await;
        probes.push(policy_probe.evidence.clone());
        if let Some(outcome) = policy_probe.classified_outcome(request, &probes) {
            return outcome;
        }

        let decisive_reason = session_probe
            .decision_hint
            .clone()
            .or(policy_probe.decision_hint.clone())
            .unwrap_or_else(|| "waiaas_authority_confirmed".to_owned());
        let authority_timestamp = session_probe
            .authority_timestamp
            .clone()
            .or(policy_probe.authority_timestamp.clone())
            .or(Some(observed_at));
        let mut capture = WaiaasAuthorityCapture::from_evidence(
            request,
            WaiaasAuthorityEvidence::pass(
                Some(request.session_id.clone()),
                Some(request.policy_id.clone()),
                decisive_reason,
                authority_timestamp,
            ),
        );
        capture.probes = probes;
        WaiaasAuthorityOutcome::pass(capture)
    }
}

#[derive(Debug, Clone)]
struct ClassifiedProbe {
    evidence: WaiaasProbeEvidence,
    outcome: Option<AuthorityDecision>,
    authority_timestamp: Option<String>,
    decision_hint: Option<String>,
}

impl ClassifiedProbe {
    fn classified_outcome(
        &self,
        request: &WaiaasAuthorityRequest,
        probes: &[WaiaasProbeEvidence],
    ) -> Option<WaiaasAuthorityOutcome> {
        let decision = self.outcome?;
        let evidence = match decision {
            AuthorityDecision::Pass => return None,
            AuthorityDecision::Blocked => WaiaasAuthorityEvidence::blocked(
                Some(request.session_id.clone()),
                Some(request.policy_id.clone()),
                self.evidence.reason_code.clone(),
                self.authority_timestamp
                    .clone()
                    .or(Some(self.evidence.observed_at.clone())),
            ),
            AuthorityDecision::Hold => WaiaasAuthorityEvidence::hold(
                Some(request.session_id.clone()),
                Some(request.policy_id.clone()),
                self.evidence.reason_code.clone(),
                self.authority_timestamp
                    .clone()
                    .or(Some(self.evidence.observed_at.clone())),
            ),
            AuthorityDecision::Fail => WaiaasAuthorityEvidence::fail(
                Some(request.session_id.clone()),
                Some(request.policy_id.clone()),
                self.evidence.reason_code.clone(),
                self.authority_timestamp
                    .clone()
                    .or(Some(self.evidence.observed_at.clone())),
            ),
        };
        let mut capture = WaiaasAuthorityCapture::from_evidence(request, evidence);
        capture.probes = probes.to_vec();
        Some(match decision {
            AuthorityDecision::Blocked => WaiaasAuthorityOutcome::blocked(capture),
            AuthorityDecision::Hold => WaiaasAuthorityOutcome::hold(capture),
            AuthorityDecision::Fail => WaiaasAuthorityOutcome::fail(capture),
            AuthorityDecision::Pass => unreachable!(),
        })
    }
}

impl HttpWaiaasAuthorityAdapter {
    async fn probe_health(&self, url: String, observed_at: String) -> ClassifiedProbe {
        match self.client.get(&url).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return ClassifiedProbe {
                        evidence: WaiaasProbeEvidence {
                            probe: "health".to_owned(),
                            url,
                            outcome: "ok".to_owned(),
                            status_code: Some(status.as_u16()),
                            reason_code: "waiaas_health_ok".to_owned(),
                            observed_at,
                            detail: None,
                        },
                        outcome: None,
                        authority_timestamp: None,
                        decision_hint: None,
                    };
                }

                let decision = classify_status("health", status);
                ClassifiedProbe {
                    evidence: WaiaasProbeEvidence {
                        probe: "health".to_owned(),
                        url,
                        outcome: decision_label(decision).to_owned(),
                        status_code: Some(status.as_u16()),
                        reason_code: format!("waiaas_health_http_{}", status.as_u16()),
                        observed_at,
                        detail: Some(format!(
                            "WAIaaS health probe returned HTTP {}",
                            status.as_u16()
                        )),
                    },
                    outcome: Some(decision),
                    authority_timestamp: None,
                    decision_hint: None,
                }
            }
            Err(error) => ClassifiedProbe {
                evidence: WaiaasProbeEvidence {
                    probe: "health".to_owned(),
                    url,
                    outcome: "fail".to_owned(),
                    status_code: None,
                    reason_code: transport_reason_code("health", &error),
                    observed_at,
                    detail: Some(error.to_string()),
                },
                outcome: Some(AuthorityDecision::Fail),
                authority_timestamp: None,
                decision_hint: None,
            },
        }
    }

    async fn probe_resource(
        &self,
        probe: &str,
        url: String,
        expected_id: String,
        observed_at: String,
    ) -> ClassifiedProbe {
        match self.client.get(&url).send().await {
            Ok(response) => {
                let status = response.status();
                let status_code = status.as_u16();
                let body = response.text().await.unwrap_or_default();
                if !status.is_success() {
                    let decision = classify_status(probe, status);
                    return ClassifiedProbe {
                        evidence: WaiaasProbeEvidence {
                            probe: probe.to_owned(),
                            url,
                            outcome: decision_label(decision).to_owned(),
                            status_code: Some(status_code),
                            reason_code: format!("waiaas_{probe}_http_{status_code}"),
                            observed_at,
                            detail: (!body.is_empty()).then_some(body),
                        },
                        outcome: Some(decision),
                        authority_timestamp: None,
                        decision_hint: None,
                    };
                }

                let parsed = serde_json::from_str::<Value>(&body).ok();
                let authority_timestamp = parsed
                    .as_ref()
                    .and_then(extract_timestamp)
                    .or(Some(observed_at.clone()));
                let decision_hint = parsed.as_ref().and_then(extract_decision_hint);
                let decision = decision_hint.as_deref().and_then(decision_from_hint);
                let returned_id = parsed.as_ref().and_then(|value| {
                    extract_string(value, &["id", probe, &format!("{}_id", probe)])
                });
                let id_matches = returned_id
                    .as_deref()
                    .is_none_or(|value| value == expected_id);
                let reason_code = if !id_matches {
                    format!("waiaas_{probe}_id_mismatch")
                } else if let Some(decision_hint) = &decision_hint {
                    normalize_reason_code(decision_hint)
                } else {
                    format!("waiaas_{probe}_ok")
                };
                let outcome = if !id_matches {
                    Some(AuthorityDecision::Fail)
                } else {
                    decision
                };
                ClassifiedProbe {
                    evidence: WaiaasProbeEvidence {
                        probe: probe.to_owned(),
                        url,
                        outcome: decision_label(outcome.unwrap_or(AuthorityDecision::Pass))
                            .to_owned(),
                        status_code: Some(status_code),
                        reason_code,
                        observed_at,
                        detail: returned_id
                            .filter(|value| value != &expected_id)
                            .map(|value| {
                                format!(
                                    "expected {probe} id `{expected_id}` but observed `{value}`"
                                )
                            }),
                    },
                    outcome,
                    authority_timestamp,
                    decision_hint,
                }
            }
            Err(error) => ClassifiedProbe {
                evidence: WaiaasProbeEvidence {
                    probe: probe.to_owned(),
                    url,
                    outcome: "fail".to_owned(),
                    status_code: None,
                    reason_code: transport_reason_code(probe, &error),
                    observed_at,
                    detail: Some(error.to_string()),
                },
                outcome: Some(AuthorityDecision::Fail),
                authority_timestamp: None,
                decision_hint: None,
            },
        }
    }
}

fn classify_status(probe: &str, status: StatusCode) -> AuthorityDecision {
    match status {
        StatusCode::ACCEPTED
        | StatusCode::LOCKED
        | StatusCode::TOO_EARLY
        | StatusCode::TOO_MANY_REQUESTS => AuthorityDecision::Hold,
        StatusCode::UNAUTHORIZED
        | StatusCode::FORBIDDEN
        | StatusCode::NOT_FOUND
        | StatusCode::CONFLICT
        | StatusCode::GONE
        | StatusCode::PRECONDITION_FAILED => AuthorityDecision::Blocked,
        _ if status.is_server_error() => AuthorityDecision::Fail,
        _ if probe == "health" => AuthorityDecision::Fail,
        _ if status.is_client_error() => AuthorityDecision::Blocked,
        _ => AuthorityDecision::Fail,
    }
}

fn decision_label(decision: AuthorityDecision) -> &'static str {
    match decision {
        AuthorityDecision::Pass => "pass",
        AuthorityDecision::Blocked => "blocked",
        AuthorityDecision::Hold => "hold",
        AuthorityDecision::Fail => "fail",
    }
}

fn transport_reason_code(probe: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("waiaas_{probe}_timeout")
    } else if error.is_connect() {
        format!("waiaas_{probe}_connect_error")
    } else {
        format!("waiaas_{probe}_request_failed")
    }
}

fn extract_timestamp(value: &Value) -> Option<String> {
    extract_string(
        value,
        &[
            "authority_timestamp",
            "timestamp",
            "observed_at",
            "updated_at",
            "decision_timestamp",
        ],
    )
}

fn extract_decision_hint(value: &Value) -> Option<String> {
    extract_string(
        value,
        &[
            "authority_decision",
            "decision",
            "status",
            "reason_code",
            "reason",
        ],
    )
}

fn extract_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(found) = value.get(*key).and_then(Value::as_str) {
            let trimmed = found.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }

    for child_key in ["data", "session", "policy", "result"] {
        if let Some(child) = value.get(child_key) {
            if let Some(found) = extract_string(child, keys) {
                return Some(found);
            }
        }
    }

    None
}

fn decision_from_hint(hint: &str) -> Option<AuthorityDecision> {
    let normalized = hint.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    if [
        "blocked",
        "deny",
        "denied",
        "reject",
        "rejected",
        "revoked",
        "expired",
        "forbidden",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        return Some(AuthorityDecision::Blocked);
    }
    if [
        "hold",
        "wait",
        "waiting",
        "pending",
        "pause",
        "paused",
        "throttle",
        "rate_limited",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        return Some(AuthorityDecision::Hold);
    }
    if ["fail", "failed", "error", "invalid"]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return Some(AuthorityDecision::Fail);
    }
    if [
        "pass",
        "allow",
        "allowed",
        "approved",
        "active",
        "authorized",
        "ready",
        "ok",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        return Some(AuthorityDecision::Pass);
    }
    None
}

fn normalize_reason_code(value: &str) -> String {
    let normalized = value
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let collapsed = normalized
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if collapsed.is_empty() {
        "waiaas_authority_unknown".to_owned()
    } else {
        collapsed
    }
}

fn now_unix_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}
