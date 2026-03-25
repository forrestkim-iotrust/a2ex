use a2ex_onboarding::StrategyRuntimeHandoffError;
use a2ex_skill_bundle::BundleError;
use rmcp::ErrorData as RmcpError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpContractError {
    #[error("invalid bundle entry url {entry_url}: {source}")]
    InvalidEntryUrl {
        entry_url: String,
        #[source]
        source: url::ParseError,
    },
    #[error(transparent)]
    Bundle(#[from] BundleError),
    #[error(transparent)]
    OnboardingBootstrap(#[from] a2ex_onboarding::InstallBootstrapError),
    #[error(transparent)]
    Onboarding(#[from] a2ex_onboarding::GuidedOnboardingError),
    #[error(transparent)]
    RouteReadiness(#[from] a2ex_onboarding::RouteReadinessError),
    #[error(transparent)]
    OnboardingStrategySelection(#[from] a2ex_onboarding::StrategySelectionError),
    #[error(transparent)]
    StrategyRuntimeHandoff(#[from] StrategyRuntimeHandoffError),
    #[error(transparent)]
    State(#[from] a2ex_state::StateError),
    #[error("reload changed session identity from {expected} to {actual}")]
    SessionIdentityChanged { expected: String, actual: String },
    #[error("unknown skill session {session_id}")]
    UnknownSession { session_id: String },
    #[error("unknown onboarding install {install_id}")]
    UnknownOnboardingInstall { install_id: String },
    #[error("unknown MCP prompt {prompt_name}")]
    UnknownPrompt { prompt_name: String },
    #[error("invalid skill session resource uri {uri}: {reason}")]
    InvalidSessionResourceUri { uri: String, reason: String },
    #[error("invalid onboarding resource uri {uri}: {reason}")]
    InvalidOnboardingResourceUri { uri: String, reason: String },
    #[error("invalid runtime control resource uri {uri}: {reason}")]
    InvalidRuntimeControlResourceUri { uri: String, reason: String },
    #[error("invalid route readiness resource uri {uri}: {reason}")]
    InvalidRouteReadinessResourceUri { uri: String, reason: String },
    #[error("invalid strategy selection resource uri {uri}: {reason}")]
    InvalidStrategySelectionResourceUri { uri: String, reason: String },
    #[error("invalid strategy runtime resource uri {uri}: {reason}")]
    InvalidStrategyRuntimeResourceUri { uri: String, reason: String },
    #[error("prompt {prompt_name} requires a string {argument_name} argument")]
    MissingPromptArgument {
        prompt_name: String,
        argument_name: &'static str,
    },
    #[error("skill session {session_id} is stopped")]
    SessionStopped { session_id: String },
    #[error(
        "venue adapters not configured — call with_venue_adapters() at server construction or configure adapters before invoking venue tools"
    )]
    VenueAdaptersNotConfigured,
    #[error("polymarket credentials not derived — call venue.derive_api_key first")]
    PolymarketCredentialsNotDerived,
    #[error("venue transport error ({venue}): {message}")]
    VenueTransport { venue: String, message: String },
    #[error("signer bridge error: {message}")]
    SignerBridgeError { message: String },
}

pub fn mcp_contract_to_rmcp_error(error: McpContractError) -> RmcpError {
    match error {
        McpContractError::UnknownSession { session_id } => RmcpError::resource_not_found(
            format!("unknown skill session {session_id}"),
            Some(serde_json::json!({ "session_id": session_id })),
        ),
        McpContractError::UnknownOnboardingInstall { install_id } => RmcpError::resource_not_found(
            format!("unknown onboarding install {install_id}"),
            Some(serde_json::json!({ "install_id": install_id })),
        ),
        McpContractError::UnknownPrompt { prompt_name } => RmcpError::invalid_params(
            format!("unknown skill prompt {prompt_name}"),
            Some(serde_json::json!({ "prompt_name": prompt_name })),
        ),
        McpContractError::InvalidEntryUrl { entry_url, source } => RmcpError::invalid_params(
            format!("invalid bundle entry url {entry_url}: {source}"),
            Some(serde_json::json!({ "entry_url": entry_url })),
        ),
        McpContractError::SessionIdentityChanged { expected, actual } => RmcpError::invalid_params(
            format!("reload changed session identity from {expected} to {actual}"),
            Some(serde_json::json!({
                "expected": expected,
                "actual": actual,
                "rejection_code": "session_identity_mismatch"
            })),
        ),
        McpContractError::InvalidSessionResourceUri { uri, reason } => RmcpError::invalid_params(
            format!("invalid skill session resource uri {uri}: {reason}"),
            Some(serde_json::json!({ "uri": uri })),
        ),
        McpContractError::InvalidOnboardingResourceUri { uri, reason } => {
            RmcpError::invalid_params(
                format!("invalid onboarding resource uri {uri}: {reason}"),
                Some(serde_json::json!({ "uri": uri })),
            )
        }
        McpContractError::InvalidRuntimeControlResourceUri { uri, reason } => {
            RmcpError::invalid_params(
                format!("invalid runtime control resource uri {uri}: {reason}"),
                Some(serde_json::json!({ "uri": uri })),
            )
        }
        McpContractError::InvalidRouteReadinessResourceUri { uri, reason } => {
            RmcpError::invalid_params(
                format!("invalid route readiness resource uri {uri}: {reason}"),
                Some(serde_json::json!({ "uri": uri })),
            )
        }
        McpContractError::InvalidStrategySelectionResourceUri { uri, reason } => {
            RmcpError::invalid_params(
                format!("invalid strategy selection resource uri {uri}: {reason}"),
                Some(serde_json::json!({ "uri": uri })),
            )
        }
        McpContractError::InvalidStrategyRuntimeResourceUri { uri, reason } => {
            RmcpError::invalid_params(
                format!("invalid strategy runtime resource uri {uri}: {reason}"),
                Some(serde_json::json!({ "uri": uri })),
            )
        }
        McpContractError::MissingPromptArgument {
            prompt_name,
            argument_name,
        } => RmcpError::invalid_params(
            format!("prompt {prompt_name} requires a string {argument_name} argument"),
            Some(serde_json::json!({
                "prompt_name": prompt_name,
                "argument_name": argument_name,
            })),
        ),
        McpContractError::SessionStopped { session_id } => RmcpError::invalid_params(
            format!("skill session {session_id} is stopped"),
            Some(
                serde_json::json!({ "session_id": session_id, "rejection_code": "session_stopped" }),
            ),
        ),
        McpContractError::Bundle(error) => {
            RmcpError::internal_error(format!("skill bundle operation failed: {error}"), None)
        }
        McpContractError::OnboardingBootstrap(error) => {
            RmcpError::internal_error(format!("onboarding bootstrap failed: {error}"), None)
        }
        McpContractError::Onboarding(a2ex_onboarding::GuidedOnboardingError::ActionRejected {
            code,
            message,
        }) => RmcpError::invalid_params(
            format!("guided onboarding action rejected: {code}: {message}"),
            Some(serde_json::json!({ "rejection_code": code, "message": message })),
        ),
        McpContractError::Onboarding(error) => {
            RmcpError::internal_error(format!("onboarding operation failed: {error}"), None)
        }
        McpContractError::RouteReadiness(
            a2ex_onboarding::RouteReadinessError::ActionRejected { code, message },
        ) => RmcpError::invalid_params(
            format!("route readiness action rejected: {code}: {message}"),
            Some(serde_json::json!({ "rejection_code": code, "message": message })),
        ),
        McpContractError::RouteReadiness(error) => {
            RmcpError::internal_error(format!("route readiness operation failed: {error}"), None)
        }
        McpContractError::OnboardingStrategySelection(
            a2ex_onboarding::StrategySelectionError::NotFound {
                install_id,
                proposal_id,
            },
        ) => RmcpError::invalid_params(
            format!("strategy selection not found for install {install_id} proposal {proposal_id}"),
            Some(serde_json::json!({
                "install_id": install_id,
                "proposal_id": proposal_id,
            })),
        ),
        McpContractError::OnboardingStrategySelection(
            a2ex_onboarding::StrategySelectionError::InvalidOverrideKey { override_key },
        ) => RmcpError::invalid_params(
            format!("invalid_override_key: {override_key}"),
            Some(serde_json::json!({ "override_key": override_key })),
        ),
        McpContractError::OnboardingStrategySelection(
            a2ex_onboarding::StrategySelectionError::StaleSelectionRevision {
                expected_selection_revision,
                actual_selection_revision,
            },
        ) => RmcpError::invalid_params(
            format!(
                "stale_selection_revision: expected {expected_selection_revision} actual {actual_selection_revision}"
            ),
            Some(serde_json::json!({
                "expected_selection_revision": expected_selection_revision,
                "actual_selection_revision": actual_selection_revision,
            })),
        ),
        McpContractError::OnboardingStrategySelection(error) => RmcpError::internal_error(
            format!("strategy selection operation failed: {error}"),
            None,
        ),
        McpContractError::StrategyRuntimeHandoff(StrategyRuntimeHandoffError::NotFound {
            install_id,
            proposal_id,
            selection_id,
        }) => RmcpError::resource_not_found(
            format!(
                "strategy runtime handoff not found for install {install_id} proposal {proposal_id} selection {selection_id}"
            ),
            Some(serde_json::json!({
                "install_id": install_id,
                "proposal_id": proposal_id,
                "selection_id": selection_id,
            })),
        ),
        McpContractError::StrategyRuntimeHandoff(error) => RmcpError::internal_error(
            format!("strategy runtime handoff operation failed: {error}"),
            None,
        ),
        McpContractError::State(error) => RmcpError::internal_error(
            format!("runtime control state operation failed: {error}"),
            None,
        ),
        McpContractError::VenueAdaptersNotConfigured => RmcpError::invalid_params(
            "venue adapters not configured — initialize the server with venue adapters before calling venue tools".to_owned(),
            Some(serde_json::json!({
                "rejection_code": "venue_adapters_not_configured",
                "hint": "construct the server via A2exSkillMcpServer::with_venue_adapters()"
            })),
        ),
        McpContractError::PolymarketCredentialsNotDerived => RmcpError::invalid_params(
            "polymarket credentials not derived — call venue.derive_api_key first".to_owned(),
            Some(serde_json::json!({
                "rejection_code": "polymarket_credentials_not_derived",
                "hint": "call venue.derive_api_key with the wallet address before trading on Polymarket"
            })),
        ),
        McpContractError::VenueTransport { venue, message } => RmcpError::internal_error(
            format!("venue transport error ({venue}): {message}"),
            Some(serde_json::json!({ "venue": venue, "error": message })),
        ),
        McpContractError::SignerBridgeError { message } => RmcpError::internal_error(
            format!("signer bridge error: {message}"),
            Some(serde_json::json!({ "error": message })),
        ),
    }
}
