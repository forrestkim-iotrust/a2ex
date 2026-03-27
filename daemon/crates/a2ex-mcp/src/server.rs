use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
use a2ex_prediction_market_adapter::{PolymarketApiCredentials, PredictionMarketAdapter};
use a2ex_signer_bridge::SignerBridge;

use a2ex_onboarding::{
    ApplyStrategySelectionOverride, ApplyStrategySelectionOverrideRequest,
    ApproveStrategySelectionRequest, BootstrapReport, GuidedOnboardingAction,
    GuidedOnboardingActionRef, GuidedOnboardingInspection, GuidedOnboardingInspectionRequest,
    GuidedOnboardingStep, InspectStrategyReportWindowRequest, InspectStrategySelectionRequest,
    InstallBootstrapRequest, MaterializeStrategySelectionRequest, OnboardingAggregateStatus,
    OnboardingBundleDrift, OnboardingChecklistItem, ProposalHandoff,
    ReopenStrategySelectionRequest, RouteReadinessAction, RouteReadinessActionRef,
    RouteReadinessActionRequest, RouteReadinessActionResult, RouteReadinessEvaluationRequest,
    RouteReadinessInspectionRequest, RouteReadinessRecord, StrategyExceptionRollup,
    StrategyOperatorReport, StrategyReportWindow, StrategyRuntimeHandoffError,
    StrategyRuntimeHandoffRecord, StrategyRuntimeMonitoringSummary, StrategySelectionApprovalInput,
    StrategySelectionInspection, StrategySelectionOverrideRecord, StrategySelectionRecord,
    apply_guided_onboarding_action, apply_route_readiness_action,
    apply_strategy_selection_override, approve_strategy_selection, bootstrap_install,
    evaluate_route_readiness, inspect_guided_onboarding, inspect_route_readiness,
    inspect_strategy_exception_rollup, inspect_strategy_operator_report,
    inspect_strategy_report_window, inspect_strategy_runtime_eligibility,
    inspect_strategy_runtime_monitoring, inspect_strategy_selection,
    materialize_strategy_selection, reopen_strategy_selection,
};
use a2ex_skill_bundle::{
    BundleDocumentLifecycleChange, BundleLifecycleChange,
    BundleLifecycleClassification, BundleLifecycleDiagnostic, InterpretationBlocker,
    InterpretationEvidence, ProposalQuantitativeCompleteness, ProposalReadiness, SkillBundle,
    SkillBundleInterpretation, SkillBundleInterpretationStatus, generate_proposal_packet,
    interpret_bundle_load_outcome, load_skill_bundle_from_url,
};
use a2ex_state::{AUTONOMOUS_RUNTIME_CONTROL_SCOPE, PersistedRuntimeControl, StateRepository};
use reqwest::Client;
use rmcp::{
    ErrorData as RmcpError, RoleServer, ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParams, CallToolResult, GetPromptRequestParams,
        GetPromptResult, Implementation, InitializeResult, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        Prompt, PromptArgument, PromptMessage, PromptMessageRole, RawResource, RawResourceTemplate,
        ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
        ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use url::Url;

use crate::error::{McpContractError, mcp_contract_to_rmcp_error};

use crate::chain_tools::{
    ChainBalanceRequest, ChainBalanceResponse, ChainExecuteRequest, ChainExecuteResponse,
    ChainReadRequest, ChainReadResponse, ChainSimulateRequest, ChainSimulateResponse,
    TOOL_CHAIN_BALANCE, TOOL_CHAIN_EXECUTE, TOOL_CHAIN_READ, TOOL_CHAIN_SIMULATE,
};
use crate::defi_tools::{
    DefiBridgeRequest, DefiBridgeResponse, DefiApproveRequest, DefiApproveResponse,
    DefiAnalyzeRequest, DefiAnalyzeResponse,
    TOOL_DEFI_BRIDGE, TOOL_DEFI_APPROVE, TOOL_DEFI_ANALYZE,
};
use crate::venue_recipes::{
    PolymarketTradeRequest, HyperliquidTradeRequest, VenueTradeResponse,
    TOOL_POLYMARKET_TRADE, TOOL_HYPERLIQUID_TRADE,
};
use crate::session::{SessionAction, SkillSessionRegistry};
use crate::venue_tools::{
    BridgeStatusRequest, BridgeStatusResponse, DeriveApiKeyRequest, DeriveApiKeyResponse,
    PrepareBridgeRequest, PrepareBridgeResponse, QueryPositionsRequest, QueryPositionsResponse,
    TOOL_VENUE_BRIDGE_STATUS, TOOL_VENUE_DERIVE_API_KEY, TOOL_VENUE_PREPARE_BRIDGE,
    TOOL_VENUE_QUERY_POSITIONS, TOOL_VENUE_TRADE_HYPERLIQUID, TOOL_VENUE_TRADE_POLYMARKET,
    TradeHyperliquidRequest, TradeHyperliquidResponse, TradePolymarketRequest,
    TradePolymarketResponse,
};

pub const SERVER_NAME: &str = "a2ex.skills";
pub const TOOL_LOAD_BUNDLE: &str = "skills_load_bundle";
pub const TOOL_RELOAD_BUNDLE: &str = "skills_reload_bundle";
pub const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills_generate_proposal_packet";
pub const TOOL_STOP_SESSION: &str = "skills_stop_session";
pub const TOOL_CLEAR_STOP: &str = "skills_clear_stop";
pub const PROMPT_STATUS_SUMMARY: &str = "skills_session_status_summary";
pub const PROMPT_OWNER_GUIDANCE: &str = "skills_owner_guidance";
pub const PROMPT_PROPOSAL_PACKET: &str = "skills_proposal_packet";
pub const PROMPT_OPERATOR_GUIDANCE: &str = "skills_operator_guidance";
pub const PROMPT_ARGUMENT_SESSION_ID: &str = "session_id";

pub const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding_bootstrap_install";
pub const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding_apply_action";
pub const TOOL_EVALUATE_ROUTE_READINESS: &str = "readiness_evaluate_route";
pub const TOOL_APPLY_ROUTE_READINESS_ACTION: &str = "readiness_apply_action";
pub const TOOL_STRATEGY_SELECTION_MATERIALIZE: &str = "strategy_selection_materialize";
pub const TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE: &str = "strategy_selection_apply_override";
pub const TOOL_STRATEGY_SELECTION_APPROVE: &str = "strategy_selection_approve";
pub const TOOL_STRATEGY_SELECTION_REOPEN: &str = "strategy_selection_reopen";
pub const TOOL_RUNTIME_STOP: &str = "runtime_stop";
pub const TOOL_RUNTIME_PAUSE: &str = "runtime_pause";
pub const TOOL_RUNTIME_CLEAR_STOP: &str = "runtime_clear_stop";
pub const PROMPT_CURRENT_STEP_GUIDANCE: &str = "onboarding_current_step_guidance";
pub const PROMPT_FAILURE_SUMMARY: &str = "onboarding_failure_summary";
pub const PROMPT_ROUTE_READINESS_GUIDANCE: &str = "readiness_route_guidance";
pub const PROMPT_ROUTE_BLOCKER_SUMMARY: &str = "readiness_route_blocker_summary";
pub const PROMPT_STRATEGY_SELECTION_GUIDANCE: &str = "strategy_selection_guidance";
pub const PROMPT_STRATEGY_SELECTION_DISCUSSION: &str = "operator_strategy_selection_discussion";
pub const PROMPT_STRATEGY_SELECTION_RECOVERY: &str = "operator_strategy_selection_recovery";
pub const PROMPT_RUNTIME_CONTROL_GUIDANCE: &str = "runtime_control_guidance";
pub const PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE: &str =
    "operator_strategy_operator_report_guidance";
pub const PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE: &str = "operator_strategy_report_window_guidance";
pub const PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";
pub const PROMPT_ARGUMENT_PROPOSAL_ID: &str = "proposal_id";
pub const PROMPT_ARGUMENT_ROUTE_ID: &str = "route_id";
pub const PROMPT_ARGUMENT_SELECTION_ID: &str = "selection_id";
const DEFAULT_REPORT_WINDOW_LIMIT: u32 = 20;

/// Holds initialised venue adapter instances for the MCP server.
///
/// Each adapter is pre-configured with its transport; the signer bridge is
/// shared across venues that need EIP-712 signing. `polymarket_credentials`
/// is populated at runtime after a successful `derive_api_key` call.
/// Holds initialised venue adapter instances that can be cloned cheaply
/// (all fields are `Arc` or `Clone` wrappers). Polymarket credentials are
/// stored per wallet address to avoid a single process-wide active context.
#[derive(Clone)]
pub struct VenueAdapters {
    pub across: AcrossAdapter,
    pub hyperliquid: HyperliquidAdapter,
    pub prediction_market: PredictionMarketAdapter,
    pub signer: Arc<dyn SignerBridge>,
    pub polymarket_credentials: Arc<RwLock<BTreeMap<String, PolymarketApiCredentials>>>,
    /// Base URL for the Polymarket CLOB API (e.g. `https://clob.polymarket.com`).
    /// Overridable for testing with wiremock.
    pub polymarket_clob_base_url: String,
}

impl std::fmt::Debug for VenueAdapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cred_status = self
            .polymarket_credentials
            .read()
            .map(|c| format!("{} wallet(s)", c.len()))
            .unwrap_or_else(|_| "poisoned".to_owned());
        f.debug_struct("VenueAdapters")
            .field("across", &"AcrossAdapter")
            .field("hyperliquid", &"HyperliquidAdapter")
            .field("prediction_market", &"PredictionMarketAdapter")
            .field("signer", &"Arc<dyn SignerBridge>")
            .field("polymarket_credentials", &cred_status)
            .field("polymarket_clob_base_url", &self.polymarket_clob_base_url)
            .finish()
    }
}

impl VenueAdapters {
    /// Default Polymarket CLOB base URL.
    pub const DEFAULT_POLYMARKET_CLOB_BASE_URL: &str = "https://clob.polymarket.com";

    /// Create a new `VenueAdapters` with no pre-existing Polymarket credentials.
    pub fn new(
        across: AcrossAdapter,
        hyperliquid: HyperliquidAdapter,
        prediction_market: PredictionMarketAdapter,
        signer: Arc<dyn SignerBridge>,
    ) -> Self {
        Self {
            across,
            hyperliquid,
            prediction_market,
            signer,
            polymarket_credentials: Arc::new(RwLock::new(BTreeMap::new())),
            polymarket_clob_base_url: Self::DEFAULT_POLYMARKET_CLOB_BASE_URL.to_owned(),
        }
    }

    /// Override the Polymarket CLOB base URL (e.g. for wiremock tests).
    pub fn with_polymarket_clob_base_url(mut self, url: impl Into<String>) -> Self {
        self.polymarket_clob_base_url = url.into();
        self
    }
}

/// Request to update the hot-wallet session token at runtime.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionRefreshRequest {
    /// New session token from WAIaaS.
    pub session_token: String,
}

#[derive(Debug, Clone)]
pub struct A2exSkillMcpServer {
    pub(crate) client: Client,
    pub(crate) sessions: SkillSessionRegistry,
    pub(crate) onboarding_installs: OnboardingInstallRegistry,
    pub(crate) venue_adapters: Arc<RwLock<Option<VenueAdapters>>>,
}

#[derive(Debug, Clone, Default)]
struct OnboardingInstallRegistry {
    installs: Arc<RwLock<BTreeMap<String, RegisteredOnboardingInstall>>>,
}

#[derive(Debug, Clone)]
struct RegisteredOnboardingInstall {
    state_db_path: PathBuf,
    workspace_id: String,
    attached_bundle_url: String,
    aggregate_status: OnboardingAggregateStatus,
}

impl OnboardingInstallRegistry {
    fn remember(
        &self,
        install_id: String,
        state_db_path: PathBuf,
        workspace_id: String,
        attached_bundle_url: String,
        aggregate_status: OnboardingAggregateStatus,
    ) {
        self.installs
            .write()
            .expect("onboarding install registry write lock")
            .insert(
                install_id,
                RegisteredOnboardingInstall {
                    state_db_path,
                    workspace_id,
                    attached_bundle_url,
                    aggregate_status,
                },
            );
    }

    fn update_status(
        &self,
        install_id: &str,
        aggregate_status: OnboardingAggregateStatus,
    ) -> Result<(), McpContractError> {
        let mut installs = self
            .installs
            .write()
            .expect("onboarding install registry write lock");
        let install = installs.get_mut(install_id).ok_or_else(|| {
            McpContractError::UnknownOnboardingInstall {
                install_id: install_id.to_owned(),
            }
        })?;
        install.aggregate_status = aggregate_status;
        Ok(())
    }

    fn ready_handoff_for_entry_url(&self, entry_url: &str) -> Option<SessionHandoff> {
        self.installs
            .read()
            .expect("onboarding install registry read lock")
            .iter()
            .find(|(_, install)| {
                install.aggregate_status == OnboardingAggregateStatus::Ready
                    && install.attached_bundle_url == entry_url
            })
            .map(|(install_id, install)| SessionHandoff {
                install_id: install_id.clone(),
                workspace_id: install.workspace_id.clone(),
                attached_bundle_url: install.attached_bundle_url.clone(),
            })
    }

    fn state_db_path(&self, install_id: &str) -> Result<PathBuf, McpContractError> {
        self.installs
            .read()
            .expect("onboarding install registry read lock")
            .get(install_id)
            .map(|install| install.state_db_path.clone())
            .ok_or_else(|| McpContractError::UnknownOnboardingInstall {
                install_id: install_id.to_owned(),
            })
    }
}

impl Default for A2exSkillMcpServer {
    fn default() -> Self {
        Self::new(Client::new())
    }
}

impl A2exSkillMcpServer {
    pub(crate) fn polymarket_wallet_key(wallet_address: &str) -> String {
        wallet_address.trim().to_ascii_lowercase()
    }

    pub fn new(client: Client) -> Self {
        Self {
            client,
            sessions: SkillSessionRegistry::default(),
            onboarding_installs: OnboardingInstallRegistry::default(),
            venue_adapters: Arc::new(RwLock::new(None)),
        }
    }

    pub fn with_registry(client: Client, sessions: SkillSessionRegistry) -> Self {
        Self {
            client,
            sessions,
            onboarding_installs: OnboardingInstallRegistry::default(),
            venue_adapters: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a server pre-configured with venue adapters for bridge, trading,
    /// and prediction-market operations.
    pub fn with_venue_adapters(adapters: VenueAdapters) -> Self {
        Self {
            client: Client::new(),
            sessions: SkillSessionRegistry::default(),
            onboarding_installs: OnboardingInstallRegistry::default(),
            venue_adapters: Arc::new(RwLock::new(Some(adapters))),
        }
    }

    /// Returns a reference to the venue adapters lock.
    pub fn venue_adapters(&self) -> &Arc<RwLock<Option<VenueAdapters>>> {
        &self.venue_adapters
    }

    /// Read the venue adapters lock and return a clone if configured.
    /// Returns `VenueAdaptersNotConfigured` if adapters have not been set.
    pub(crate) fn get_venue_adapters(&self) -> Result<VenueAdapters, McpContractError> {
        let guard = self
            .venue_adapters
            .read()
            .expect("venue adapters read lock");
        guard
            .as_ref()
            .cloned()
            .ok_or(McpContractError::VenueAdaptersNotConfigured)
    }

    // -----------------------------------------------------------------------
    // Shared helpers
    // -----------------------------------------------------------------------

    pub(crate) fn resolve_hot_wallet_address(&self) -> Result<String, String> {
        std::env::var("A2EX_HOT_WALLET_ADDRESS")
            .map_err(|_| "A2EX_HOT_WALLET_ADDRESS not set — plugin bootstrap must provide this".into())
            .and_then(|a| {
                if a.is_empty() {
                    Err("A2EX_HOT_WALLET_ADDRESS is empty".into())
                } else {
                    Ok(a)
                }
            })
    }

    // Layer 1 chain handlers moved to handlers/chain.rs


    // Layer 2 DeFi handlers moved to handlers/defi.rs
    // Layer 3 venue recipe handlers moved to handlers/venue_recipe.rs


    // Venue tool handlers moved to handlers/venue.rs

    pub fn session_registry(&self) -> &SkillSessionRegistry {
        &self.sessions
    }

    pub async fn bootstrap_install(
        &self,
        request: OnboardingBootstrapInstallRequest,
    ) -> Result<OnboardingBootstrapInstallResponse, McpContractError> {
        let result = bootstrap_install(InstallBootstrapRequest {
            install_url: parse_entry_url(&request.install_url)?,
            workspace_root: request.workspace_root,
            expected_workspace_id: request.expected_workspace_id,
            expected_install_id: request.expected_install_id,
        })
        .await
        .map_err(McpContractError::OnboardingBootstrap)?;

        let state_db_path = result.bootstrap.state_db_path.clone();
        self.onboarding_installs.remember(
            result.install_id.clone(),
            state_db_path.clone(),
            result.workspace_id.clone(),
            result.attached_bundle_url.to_string(),
            result.onboarding.aggregate_status,
        );
        let inspection = inspect_guided_onboarding(GuidedOnboardingInspectionRequest {
            state_db_path,
            install_id: result.install_id.clone(),
        })
        .await
        .map_err(McpContractError::Onboarding)?;

        Ok(OnboardingBootstrapInstallResponse::from_parts(
            result, inspection,
        ))
    }

    pub async fn apply_onboarding_action(
        &self,
        request: OnboardingApplyActionRequest,
    ) -> Result<OnboardingApplyActionResponse, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        let install_id = request.install_id.clone();
        let result =
            apply_guided_onboarding_action(a2ex_onboarding::GuidedOnboardingActionRequest {
                state_db_path,
                install_id: request.install_id,
                action: request.action,
            })
            .await
            .map_err(McpContractError::Onboarding)?;
        self.onboarding_installs
            .update_status(&install_id, result.aggregate_status)?;

        Ok(OnboardingApplyActionResponse::from(result))
    }

    pub async fn read_onboarding_inspection(
        &self,
        install_id: &str,
    ) -> Result<GuidedOnboardingInspection, McpContractError> {
        let state_db_path = self.onboarding_installs.state_db_path(install_id)?;
        inspect_guided_onboarding(GuidedOnboardingInspectionRequest {
            state_db_path,
            install_id: install_id.to_owned(),
        })
        .await
        .map_err(McpContractError::Onboarding)
    }

    pub async fn evaluate_route_readiness(
        &self,
        request: EvaluateRouteReadinessRequest,
    ) -> Result<RouteReadinessRecord, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        evaluate_route_readiness(RouteReadinessEvaluationRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            route_id: request.route_id,
            request_id: request.request_id,
        })
        .await
        .map_err(McpContractError::RouteReadiness)
    }

    pub async fn read_route_readiness(
        &self,
        request: ReadRouteReadinessRequest,
    ) -> Result<RouteReadinessRecord, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        inspect_route_readiness(RouteReadinessInspectionRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            route_id: request.route_id,
        })
        .await
        .map_err(McpContractError::RouteReadiness)
    }

    pub async fn apply_route_readiness_action(
        &self,
        request: ApplyRouteReadinessActionRequest,
    ) -> Result<ApplyRouteReadinessActionResponse, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        apply_route_readiness_action(RouteReadinessActionRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            route_id: request.route_id,
            action: request.action,
        })
        .await
        .map(ApplyRouteReadinessActionResponse::from)
        .map_err(McpContractError::RouteReadiness)
    }

    pub async fn materialize_strategy_selection_record(
        &self,
        request: StrategySelectionMaterializeRequest,
    ) -> Result<StrategySelectionMutationResponse, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        let snapshot = self.sessions.get(&request.proposal_id)?;
        let proposal = snapshot.proposal_packet()?;
        let record = materialize_strategy_selection(MaterializeStrategySelectionRequest {
            state_db_path,
            install_id: request.install_id.clone(),
            proposal_id: request.proposal_id.clone(),
            proposal_uri: snapshot.proposal_uri(),
            proposal_revision: snapshot.proposal_revision(),
            proposal,
        })
        .await
        .map_err(McpContractError::OnboardingStrategySelection)?;

        Ok(StrategySelectionMutationResponse::from_record(&record))
    }

    pub async fn apply_strategy_selection_override_record(
        &self,
        request: StrategySelectionApplyOverrideRequest,
    ) -> Result<StrategySelectionMutationResponse, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        let inspection = apply_strategy_selection_override(ApplyStrategySelectionOverrideRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
            override_record: ApplyStrategySelectionOverride {
                key: request.r#override.key,
                value: request.r#override.value,
                rationale: request.r#override.rationale,
                provenance: request.r#override.provenance,
            },
        })
        .await
        .map_err(McpContractError::OnboardingStrategySelection)?;

        Ok(StrategySelectionMutationResponse::from_record(
            &inspection.summary,
        ))
    }

    pub async fn approve_strategy_selection_record(
        &self,
        request: StrategySelectionApproveRequest,
    ) -> Result<StrategySelectionMutationResponse, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        let record = approve_strategy_selection(ApproveStrategySelectionRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
            expected_selection_revision: request.expected_selection_revision,
            approval: StrategySelectionApprovalInput {
                approved_by: request.approval.approved_by,
                note: request.approval.note,
            },
        })
        .await
        .map_err(McpContractError::OnboardingStrategySelection)?;

        Ok(StrategySelectionMutationResponse::from_record(&record))
    }

    pub async fn reopen_strategy_selection_record(
        &self,
        request: StrategySelectionReopenRequest,
    ) -> Result<StrategySelectionReopenResponse, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        let record = reopen_strategy_selection(ReopenStrategySelectionRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
            reason: request.reason,
        })
        .await
        .map_err(McpContractError::OnboardingStrategySelection)?;

        Ok(StrategySelectionReopenResponse::from_record(&record))
    }

    pub async fn inspect_strategy_selection(
        &self,
        request: StrategySelectionReadRequest,
    ) -> Result<StrategySelectionInspection, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        let inspection = inspect_strategy_selection(InspectStrategySelectionRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
        })
        .await
        .map_err(McpContractError::OnboardingStrategySelection)?;

        if inspection.summary.selection_id != request.selection_id {
            return Err(McpContractError::OnboardingStrategySelection(
                a2ex_onboarding::StrategySelectionError::NotFound {
                    install_id: inspection.summary.install_id,
                    proposal_id: inspection.summary.proposal_id,
                },
            ));
        }

        Ok(inspection)
    }

    pub async fn inspect_strategy_runtime_eligibility(
        &self,
        request: StrategySelectionReadRequest,
    ) -> Result<StrategyRuntimeHandoffRecord, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        inspect_strategy_runtime_eligibility(a2ex_onboarding::InspectStrategyRuntimeRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
        })
        .await
        .map_err(McpContractError::StrategyRuntimeHandoff)
    }

    pub async fn inspect_strategy_operator_report(
        &self,
        request: StrategySelectionReadRequest,
    ) -> Result<StrategyOperatorReport, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        inspect_strategy_operator_report(a2ex_onboarding::InspectStrategyRuntimeRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
        })
        .await
        .map_err(McpContractError::StrategyRuntimeHandoff)
    }

    pub async fn inspect_strategy_exception_rollup(
        &self,
        request: StrategySelectionReadRequest,
    ) -> Result<StrategyExceptionRollup, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        inspect_strategy_exception_rollup(a2ex_onboarding::InspectStrategyRuntimeRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
        })
        .await
        .map_err(McpContractError::StrategyRuntimeHandoff)
    }

    pub async fn inspect_strategy_report_window(
        &self,
        request: StrategySelectionReadRequest,
        cursor: String,
    ) -> Result<StrategyReportWindow, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        inspect_strategy_report_window(InspectStrategyReportWindowRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
            cursor,
            window_limit: DEFAULT_REPORT_WINDOW_LIMIT,
        })
        .await
        .map_err(McpContractError::StrategyRuntimeHandoff)
    }

    pub async fn inspect_strategy_runtime_monitoring(
        &self,
        request: StrategySelectionReadRequest,
    ) -> Result<StrategyRuntimeMonitoringSummary, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        inspect_strategy_runtime_monitoring(a2ex_onboarding::InspectStrategyRuntimeRequest {
            state_db_path,
            install_id: request.install_id,
            proposal_id: request.proposal_id,
            selection_id: request.selection_id,
        })
        .await
        .map_err(McpContractError::StrategyRuntimeHandoff)
    }

    pub async fn inspect_runtime_control(
        &self,
        install_id: &str,
    ) -> Result<PersistedRuntimeControl, McpContractError> {
        let state_db_path = self.onboarding_installs.state_db_path(install_id)?;
        let repository = StateRepository::open(&state_db_path).await?;
        Ok(repository
            .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
            .await?
            .unwrap_or_else(default_runtime_control_record))
    }

    async fn latest_runtime_selection_request(
        &self,
        install_id: &str,
    ) -> Result<Option<StrategySelectionReadRequest>, McpContractError> {
        let state_db_path = self.onboarding_installs.state_db_path(install_id)?;
        let repository = StateRepository::open(&state_db_path).await?;
        Ok(repository
            .load_latest_strategy_runtime_handoff_for_install(install_id)
            .await?
            .map(|handoff| StrategySelectionReadRequest {
                install_id: handoff.install_id,
                proposal_id: handoff.proposal_id,
                selection_id: handoff.selection_id,
            }))
    }

    pub async fn stop_runtime(
        &self,
        request: RuntimeControlMutationRequest,
    ) -> Result<RuntimeControlMutationResponse, McpContractError> {
        let record = self
            .set_runtime_control_mode(
                &request.install_id,
                "stopped",
                request.reason.as_deref().unwrap_or("operator_stop"),
                request.source.as_deref().unwrap_or("mcp.runtime_stop"),
                request
                    .observed_at
                    .as_deref()
                    .unwrap_or("1970-01-01T00:00:00Z"),
            )
            .await?;
        Ok(RuntimeControlMutationResponse::from_record(
            &request.install_id,
            TOOL_RUNTIME_STOP,
            record,
        ))
    }

    pub async fn pause_runtime(
        &self,
        request: RuntimeControlMutationRequest,
    ) -> Result<RuntimeControlMutationResponse, McpContractError> {
        let record = self
            .set_runtime_control_mode(
                &request.install_id,
                "paused",
                request.reason.as_deref().unwrap_or("operator_pause"),
                request.source.as_deref().unwrap_or("mcp.runtime_pause"),
                request
                    .observed_at
                    .as_deref()
                    .unwrap_or("1970-01-01T00:00:00Z"),
            )
            .await?;
        Ok(RuntimeControlMutationResponse::from_record(
            &request.install_id,
            TOOL_RUNTIME_PAUSE,
            record,
        ))
    }

    pub async fn clear_runtime_stop(
        &self,
        request: RuntimeControlMutationRequest,
    ) -> Result<RuntimeControlMutationResponse, McpContractError> {
        let state_db_path = self
            .onboarding_installs
            .state_db_path(&request.install_id)?;
        let repository = StateRepository::open(&state_db_path).await?;
        let mut record = repository
            .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
            .await?
            .unwrap_or_else(default_runtime_control_record);
        let observed_at = request
            .observed_at
            .as_deref()
            .unwrap_or("1970-01-01T00:00:00Z");
        let reason = request.reason.as_deref().unwrap_or("operator_clear_stop");
        let source = request
            .source
            .as_deref()
            .unwrap_or("mcp.runtime_clear_stop");
        record.control_mode = "active".to_owned();
        record.transition_reason = reason.to_owned();
        record.transition_source = source.to_owned();
        record.transitioned_at = observed_at.to_owned();
        record.last_cleared_at = Some(observed_at.to_owned());
        record.last_cleared_reason = Some(reason.to_owned());
        record.last_cleared_source = Some(source.to_owned());
        record.updated_at = observed_at.to_owned();
        repository.persist_runtime_control(&record).await?;
        Ok(RuntimeControlMutationResponse::from_record(
            &request.install_id,
            TOOL_RUNTIME_CLEAR_STOP,
            record,
        ))
    }

    async fn set_runtime_control_mode(
        &self,
        install_id: &str,
        control_mode: &str,
        reason: &str,
        source: &str,
        observed_at: &str,
    ) -> Result<PersistedRuntimeControl, McpContractError> {
        let state_db_path = self.onboarding_installs.state_db_path(install_id)?;
        let repository = StateRepository::open(&state_db_path).await?;
        let mut record = repository
            .load_runtime_control(AUTONOMOUS_RUNTIME_CONTROL_SCOPE)
            .await?
            .unwrap_or_else(default_runtime_control_record);
        record.control_mode = control_mode.to_owned();
        record.transition_reason = reason.to_owned();
        record.transition_source = source.to_owned();
        record.transitioned_at = observed_at.to_owned();
        if control_mode == "stopped"
            && record.last_rejection_code.as_deref() != Some("runtime_stopped")
        {
            record.last_rejection_code = Some("runtime_stopped".to_owned());
            record.last_rejection_message = Some(
                "runtime is stopped; clear_stop before autonomous operation can resume".to_owned(),
            );
            record.last_rejection_operation = Some("autonomous_runtime".to_owned());
            record.last_rejection_at = Some(observed_at.to_owned());
        }
        record.updated_at = observed_at.to_owned();
        repository.persist_runtime_control(&record).await?;
        Ok(record)
    }

    pub async fn initialize(&self) -> Result<SkillSurfaceContract, McpContractError> {
        Ok(skill_surface_contract())
    }

    pub async fn load_bundle(
        &self,
        request: LoadBundleRequest,
    ) -> Result<LoadBundleResponse, McpContractError> {
        self.run_bundle_action(SessionAction::Load, request.entry_url)
            .await
    }

    pub async fn reload_bundle(
        &self,
        request: ReloadBundleRequest,
    ) -> Result<LoadBundleResponse, McpContractError> {
        let existing = self.sessions.get(&request.session_id)?;
        if existing.entry_url != request.entry_url {
            let actual = stable_session_id(&request.entry_url);
            let message = format!(
                "reload entry_url {} does not match the existing session entry_url {}",
                request.entry_url, existing.entry_url
            );
            self.sessions.record_command_rejection(
                &request.session_id,
                TOOL_RELOAD_BUNDLE,
                "session_identity_mismatch",
                message,
            )?;
            return Err(McpContractError::SessionIdentityChanged {
                expected: request.session_id,
                actual,
            });
        }

        let response = self
            .run_bundle_action(SessionAction::Reload, request.entry_url)
            .await?;

        if response.session_id != request.session_id {
            self.sessions.record_command_rejection(
                &request.session_id,
                TOOL_RELOAD_BUNDLE,
                "session_identity_mismatch",
                format!(
                    "reload changed session identity from {} to {}",
                    request.session_id, response.session_id
                ),
            )?;
            return Err(McpContractError::SessionIdentityChanged {
                expected: request.session_id,
                actual: response.session_id,
            });
        }

        Ok(response)
    }

    pub async fn stop_session(
        &self,
        request: StopSessionRequest,
    ) -> Result<SessionControlResponse, McpContractError> {
        let snapshot = self.sessions.stop_session(&request.session_id)?;
        Ok(SessionControlResponse::from_snapshot(&snapshot))
    }

    pub async fn clear_stop(
        &self,
        request: ClearStopRequest,
    ) -> Result<SessionControlResponse, McpContractError> {
        let snapshot = self.sessions.clear_stop(&request.session_id)?;
        Ok(SessionControlResponse::from_snapshot(&snapshot))
    }

    pub async fn generate_proposal_packet(
        &self,
        request: GenerateProposalPacketRequest,
    ) -> Result<GenerateProposalPacketResponse, McpContractError> {
        let snapshot = self.sessions.get(&request.session_id)?;
        if snapshot.is_stopped() {
            self.sessions.record_command_rejection(
                &request.session_id,
                TOOL_GENERATE_PROPOSAL_PACKET,
                "session_stopped",
                "session is stopped; clear_stop before generating a proposal packet",
            )?;
            return Err(McpContractError::SessionStopped {
                session_id: request.session_id,
            });
        }
        let proposal = snapshot.proposal_packet()?;
        if let Some(handoff) = &snapshot.handoff {
            let state_db_path = self
                .onboarding_installs
                .state_db_path(&handoff.install_id)?;
            materialize_strategy_selection(MaterializeStrategySelectionRequest {
                state_db_path,
                install_id: handoff.install_id.clone(),
                proposal_id: snapshot.session_id.clone(),
                proposal_uri: snapshot.proposal_uri(),
                proposal_revision: snapshot.proposal_revision(),
                proposal: proposal.clone(),
            })
            .await
            .map_err(McpContractError::OnboardingStrategySelection)?;
        }

        Ok(GenerateProposalPacketResponse {
            session_id: snapshot.session_id.clone(),
            session_uri_root: snapshot.session_uri_root.clone(),
            proposal_uri: snapshot.proposal_uri(),
            revision: snapshot.revision,
            proposal_revision: snapshot.proposal_revision(),
            status: SessionInterpretationStatus::from(snapshot.status()),
            proposal_readiness: SessionProposalReadiness::from(proposal.proposal_readiness),
            blocker_count: snapshot.blocker_count(),
            ambiguity_count: snapshot.ambiguity_count(),
            capital_profile_completeness: SessionProposalCompleteness::from(
                proposal.capital_profile.completeness,
            ),
            cost_profile_completeness: SessionProposalCompleteness::from(
                proposal.cost_profile.completeness,
            ),
        })
    }

    pub async fn read_resource(
        &self,
        request: ReadSessionResourceRequest,
    ) -> Result<serde_json::Value, McpContractError> {
        let snapshot = self.sessions.get(&request.session_id)?;
        snapshot
            .resource_payload(request.resource)
            .map_err(Into::into)
    }

    pub async fn render_prompt(
        &self,
        request: RenderPromptRequest,
    ) -> Result<RenderPromptResponse, McpContractError> {
        let snapshot = self.sessions.get(&request.session_id)?;
        let referenced_resources = prompt_resource_kinds(&request.prompt_name)
            .ok_or_else(|| McpContractError::UnknownPrompt {
                prompt_name: request.prompt_name.clone(),
            })?
            .into_iter()
            .map(|resource| resource.uri_for_session(&snapshot.session_id))
            .collect::<Vec<_>>();

        let content = match request.prompt_name.as_str() {
            PROMPT_STATUS_SUMMARY => render_status_summary(&snapshot),
            PROMPT_OWNER_GUIDANCE => render_owner_guidance(&snapshot),
            PROMPT_PROPOSAL_PACKET => render_proposal_prompt(&snapshot)?,
            PROMPT_OPERATOR_GUIDANCE => render_operator_guidance(&snapshot)?,
            _ => unreachable!("validated prompt name above"),
        };

        Ok(RenderPromptResponse {
            name: request.prompt_name,
            session_id: snapshot.session_id,
            referenced_resources,
            content,
        })
    }

    async fn run_bundle_action(
        &self,
        action: SessionAction,
        entry_url: String,
    ) -> Result<LoadBundleResponse, McpContractError> {
        let entry_url = parse_entry_url(&entry_url)?;
        let handoff = self
            .onboarding_installs
            .ready_handoff_for_entry_url(entry_url.as_str());
        let outcome = load_skill_bundle_from_url(&self.client, entry_url.clone()).await?;
        let interpretation = apply_ready_onboarding_handoff(
            normalize_interpretation(interpret_bundle_load_outcome(&outcome)?),
            handoff.as_ref(),
        );
        let snapshot = self.sessions.upsert(
            action,
            entry_url.to_string(),
            outcome,
            interpretation,
            handoff,
        );

        Ok(LoadBundleResponse {
            session_id: snapshot.session_id.clone(),
            entry_url: snapshot.entry_url.clone(),
            session_uri_root: snapshot.session_uri_root.clone(),
            resource_uris: SkillSessionResourceKind::all()
                .into_iter()
                .map(|resource| resource.uri_for_session(&snapshot.session_id))
                .collect(),
            prompt_names: vec![
                PROMPT_STATUS_SUMMARY.to_owned(),
                PROMPT_OWNER_GUIDANCE.to_owned(),
                PROMPT_PROPOSAL_PACKET.to_owned(),
                PROMPT_OPERATOR_GUIDANCE.to_owned(),
            ],
            status: SessionInterpretationStatus::from(snapshot.status()),
            blocker_count: snapshot.blocker_count(),
            ambiguity_count: snapshot.ambiguity_count(),
        })
    }

    fn server_info() -> ServerInfo {
        InitializeResult::new(server_capabilities())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Load a skill bundle URL into a stable MCP session, inspect resources under \
                 a2ex://skills/sessions/{session_id}/..., call skills.generate_proposal_packet \
                 for proposal metadata, use onboarding.bootstrap_install to reopen canonical \
                 install state, inspect autonomous runtime control under \
                 a2ex://runtime/control/{install_id}/..., use runtime.stop/runtime.pause/runtime.clear_stop \
                 for explicit runtime control, use readiness.evaluate_route to refresh canonical \
                 route readiness, use readiness.apply_action for owner-progress mutation, inspect \
                 summary/progress/blocker readiness resources under \
                 a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/..., and reread \
                 canonical strategy report windows plus exception rollups under \
                 a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/report-window/{cursor} \
                 and /exception-rollup rather than trusting prior mutation receipts.",
            )
    }

    pub fn list_tools_result() -> ListToolsResult {
        ListToolsResult::with_all_items(vec![
            Tool::new(
                TOOL_LOAD_BUNDLE,
                "Load a skill bundle URL into a session-oriented MCP surface.",
                serde_json::Map::new(),
            )
            .with_input_schema::<LoadBundleRequest>()
            .with_output_schema::<LoadBundleResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Load skill bundle")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_RELOAD_BUNDLE,
                "Reload an existing skill bundle session without changing its identity.",
                serde_json::Map::new(),
            )
            .with_input_schema::<ReloadBundleRequest>()
            .with_output_schema::<LoadBundleResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Reload skill bundle")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_GENERATE_PROPOSAL_PACKET,
                "Generate stable proposal metadata for an existing skill bundle session.",
                serde_json::Map::new(),
            )
            .with_input_schema::<GenerateProposalPacketRequest>()
            .with_output_schema::<GenerateProposalPacketResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Generate proposal packet")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_STOP_SESSION,
                "Stop an intake session while preserving operator-state and failure resources.",
                serde_json::Map::new(),
            )
            .with_input_schema::<StopSessionRequest>()
            .with_output_schema::<SessionControlResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Stop session")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_CLEAR_STOP,
                "Clear a stopped intake session without changing its stable identity.",
                serde_json::Map::new(),
            )
            .with_input_schema::<ClearStopRequest>()
            .with_output_schema::<SessionControlResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Clear session stop")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_RUNTIME_STOP,
                "Stop the canonical autonomous runtime and keep blocked-state diagnostics inspectable.",
                serde_json::Map::new(),
            )
            .with_input_schema::<RuntimeControlMutationRequest>()
            .with_output_schema::<RuntimeControlMutationResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Stop runtime")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_RUNTIME_PAUSE,
                "Pause the canonical autonomous runtime so no new autonomous actions begin.",
                serde_json::Map::new(),
            )
            .with_input_schema::<RuntimeControlMutationRequest>()
            .with_output_schema::<RuntimeControlMutationResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Pause runtime")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_RUNTIME_CLEAR_STOP,
                "Clear paused or stopped autonomous runtime control without erasing failure evidence.",
                serde_json::Map::new(),
            )
            .with_input_schema::<RuntimeControlMutationRequest>()
            .with_output_schema::<RuntimeControlMutationResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Clear runtime stop")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_BOOTSTRAP_INSTALL,
                "Bootstrap or reopen a canonical onboarding install and return install-scoped MCP resources.",
                serde_json::Map::new(),
            )
            .with_input_schema::<OnboardingBootstrapInstallRequest>()
            .with_annotations(
                ToolAnnotations::with_title("Bootstrap onboarding install")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_APPLY_ONBOARDING_ACTION,
                "Apply a supported guided onboarding action against the canonical install state.",
                serde_json::Map::new(),
            )
            .with_input_schema::<OnboardingApplyActionRequest>()
            .with_annotations(
                ToolAnnotations::with_title("Apply onboarding action")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_EVALUATE_ROUTE_READINESS,
                "Evaluate or refresh canonical route readiness for one install/proposal/route handoff.",
                serde_json::Map::new(),
            )
            .with_input_schema::<EvaluateRouteReadinessRequest>()
            .with_annotations(
                ToolAnnotations::with_title("Evaluate route readiness")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_APPLY_ROUTE_READINESS_ACTION,
                "Apply a guided route-readiness owner action against canonical route-scoped state.",
                serde_json::Map::new(),
            )
            .with_input_schema::<ApplyRouteReadinessActionRequest>()
            .with_annotations(
                ToolAnnotations::with_title("Apply route readiness action")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_STRATEGY_SELECTION_MATERIALIZE,
                "Materialize or reread the canonical strategy-selection record for one install/proposal handoff.",
                serde_json::Map::new(),
            )
            .with_input_schema::<StrategySelectionMaterializeRequest>()
            .with_output_schema::<StrategySelectionMutationResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Materialize strategy selection")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE,
                "Apply a typed owner override to the canonical strategy-selection record.",
                serde_json::Map::new(),
            )
            .with_input_schema::<StrategySelectionApplyOverrideRequest>()
            .with_output_schema::<StrategySelectionMutationResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Apply strategy-selection override")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_STRATEGY_SELECTION_APPROVE,
                "Approve an exact canonical strategy-selection revision without replaying prior tool output.",
                serde_json::Map::new(),
            )
            .with_input_schema::<StrategySelectionApproveRequest>()
            .with_output_schema::<StrategySelectionMutationResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Approve strategy selection")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(false),
            ),
            Tool::new(
                TOOL_STRATEGY_SELECTION_REOPEN,
                "Reopen the canonical strategy-selection identity and return read-first operator resource URIs.",
                serde_json::Map::new(),
            )
            .with_input_schema::<StrategySelectionReopenRequest>()
            .with_output_schema::<StrategySelectionReopenResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Reopen strategy selection")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
            // ----- venue.* tools -----
            Tool::new(
                TOOL_VENUE_PREPARE_BRIDGE,
                "Prepare an Across bridge quote and return calldata + approval metadata for local signing. Submit the returned transactions via waiaas.call_contract.",
                serde_json::Map::new(),
            )
            .with_input_schema::<PrepareBridgeRequest>()
            .with_output_schema::<PrepareBridgeResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Prepare bridge")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_VENUE_TRADE_POLYMARKET,
                "Place an order on Polymarket. The daemon signs and submits the order internally, returning the final result.",
                serde_json::Map::new(),
            )
            .with_input_schema::<TradePolymarketRequest>()
            .with_output_schema::<TradePolymarketResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Trade Polymarket")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_VENUE_TRADE_HYPERLIQUID,
                "Place an order on Hyperliquid perpetual exchange. The daemon signs and submits the order internally, returning the final result.",
                serde_json::Map::new(),
            )
            .with_input_schema::<TradeHyperliquidRequest>()
            .with_output_schema::<TradeHyperliquidResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Trade Hyperliquid")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_VENUE_QUERY_POSITIONS,
                "Query open positions across configured venues, optionally filtered by venue name.",
                serde_json::Map::new(),
            )
            .with_input_schema::<QueryPositionsRequest>()
            .with_output_schema::<QueryPositionsResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Query positions")
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_VENUE_BRIDGE_STATUS,
                "Check the status of an Across bridge transfer by deposit ID.",
                serde_json::Map::new(),
            )
            .with_input_schema::<BridgeStatusRequest>()
            .with_output_schema::<BridgeStatusResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Bridge status")
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_VENUE_DERIVE_API_KEY,
                "Derive a venue API key from a wallet address for Polymarket CLOB credential setup. No secrets are returned in the response.",
                serde_json::Map::new(),
            )
            .with_input_schema::<DeriveApiKeyRequest>()
            .with_output_schema::<DeriveApiKeyResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Derive API key")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            // ----- Layer 3: Venue Recipes -----
            Tool::new(
                TOOL_POLYMARKET_TRADE,
                "Place an order on Polymarket prediction market. Handles everything automatically: bridging USDC to Polygon, gas, approval, credential derivation, and order placement. Just specify market, side, size, and price.",
                serde_json::Map::new(),
            )
            .with_input_schema::<PolymarketTradeRequest>()
            .with_output_schema::<VenueTradeResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Polymarket trade")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_HYPERLIQUID_TRADE,
                "Place an order on Hyperliquid perpetual exchange. Handles deposit and signing automatically.",
                serde_json::Map::new(),
            )
            .with_input_schema::<HyperliquidTradeRequest>()
            .with_output_schema::<VenueTradeResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Hyperliquid trade")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            // ----- Layer 2: DeFi Primitives -----
            Tool::new(
                TOOL_DEFI_BRIDGE,
                "Bridge tokens cross-chain. Handles gas, approval, deposit, and fill polling automatically. Just specify source, destination, token, and amount.",
                serde_json::Map::new(),
            )
            .with_input_schema::<DefiBridgeRequest>()
            .with_output_schema::<DefiBridgeResponse>()
            .with_annotations(
                ToolAnnotations::with_title("DeFi bridge")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_DEFI_APPROVE,
                "Approve an ERC-20 token for spending by a contract. Defaults to unlimited approval.",
                serde_json::Map::new(),
            )
            .with_input_schema::<DefiApproveRequest>()
            .with_output_schema::<DefiApproveResponse>()
            .with_annotations(
                ToolAnnotations::with_title("DeFi approve")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_DEFI_ANALYZE,
                "Analyze a smart contract. Checks verification status, fetches ABI/function list, and assesses risk level. Use before interacting with unknown contracts.",
                serde_json::Map::new(),
            )
            .with_input_schema::<DefiAnalyzeRequest>()
            .with_output_schema::<DefiAnalyzeResponse>()
            .with_annotations(
                ToolAnnotations::with_title("DeFi analyze contract")
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            // ----- Layer 1: Chain Primitives -----
            Tool::new(
                TOOL_CHAIN_READ,
                "Read on-chain data via eth_call. Use for view functions, balance queries, allowance checks.",
                serde_json::Map::new(),
            )
            .with_input_schema::<ChainReadRequest>()
            .with_output_schema::<ChainReadResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Chain read")
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_CHAIN_EXECUTE,
                "Execute an on-chain transaction. Auto-simulates before sending. Use for token transfers, contract calls, approvals.",
                serde_json::Map::new(),
            )
            .with_input_schema::<ChainExecuteRequest>()
            .with_output_schema::<ChainExecuteResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Chain execute")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(false)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_CHAIN_BALANCE,
                "Query native + ERC-20 token balances for an address on any chain.",
                serde_json::Map::new(),
            )
            .with_input_schema::<ChainBalanceRequest>()
            .with_output_schema::<ChainBalanceResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Chain balance")
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                TOOL_CHAIN_SIMULATE,
                "Simulate a transaction without executing. Returns success/fail and gas estimate.",
                serde_json::Map::new(),
            )
            .with_input_schema::<ChainSimulateRequest>()
            .with_output_schema::<ChainSimulateResponse>()
            .with_annotations(
                ToolAnnotations::with_title("Chain simulate")
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(true),
            ),
            // ----- Session management -----
            Tool::new(
                "session.refresh",
                "Update the hot-wallet session token at runtime without restarting the process.",
                serde_json::Map::new(),
            )
            .with_input_schema::<SessionRefreshRequest>()
            .with_annotations(
                ToolAnnotations::with_title("Refresh session token")
                    .read_only(false)
                    .destructive(false)
                    .idempotent(true)
                    .open_world(false),
            ),
        ])
    }

    fn list_resource_templates_result() -> ListResourceTemplatesResult {
        let mut resources = SkillSessionResourceKind::all()
            .into_iter()
            .map(|resource| {
                RawResourceTemplate::new(resource.template(), resource.display_name())
                    .with_title(format!("Skill session {}", resource.as_path_segment()))
                    .with_description(format!(
                        "Read the {} view for a skill MCP session.",
                        resource.as_path_segment()
                    ))
                    .with_mime_type("application/json")
                    .no_annotation()
            })
            .collect::<Vec<_>>();
        resources.extend(OnboardingResourceKind::all().into_iter().map(|resource| {
            RawResourceTemplate::new(resource.template(), resource.display_name())
                .with_title(format!("Onboarding install {}", resource.as_path_segment()))
                .with_description(format!(
                    "Read the {} view for a canonical onboarding install.",
                    resource.as_path_segment()
                ))
                .with_mime_type("application/json")
                .no_annotation()
        }));
        resources.extend(RuntimeControlResourceKind::all().into_iter().map(|resource| {
            RawResourceTemplate::new(resource.template(), resource.display_name())
                .with_title(format!("Runtime control {}", resource.as_path_segment()))
                .with_description(format!(
                    "Read the {} view for canonical autonomous runtime control keyed by install identity.",
                    resource.as_path_segment()
                ))
                .with_mime_type("application/json")
                .no_annotation()
        }));
        resources.extend(RouteReadinessResourceKind::all().into_iter().map(|resource| {
            RawResourceTemplate::new(resource.template(), resource.display_name())
                .with_title(format!("Route readiness {}", resource.as_path_segment()))
                .with_description(format!(
                    "Read the {} view for canonical route readiness keyed by install/proposal/route identity.",
                    resource.as_path_segment()
                ))
                .with_mime_type("application/json")
                .no_annotation()
        }));
        resources.extend(StrategySelectionResourceKind::all().into_iter().map(|resource| {
            RawResourceTemplate::new(resource.template(), resource.display_name())
                .with_title(format!("Strategy selection {}", resource.as_path_segment()))
                .with_description(format!(
                    "Read the {} view for canonical strategy selection keyed by install/proposal/selection identity.",
                    resource.as_path_segment()
                ))
                .with_mime_type("application/json")
                .no_annotation()
        }));
        resources.extend(StrategyRuntimeResourceKind::all().into_iter().map(|resource| {
            RawResourceTemplate::new(resource.template(), resource.display_name())
                .with_title(format!("Strategy runtime {}", resource.as_path_segment()))
                .with_description(format!(
                    "Read the {} view for canonical approved-runtime inspection keyed by install/proposal/selection identity.",
                    resource.as_path_segment()
                ))
                .with_mime_type("application/json")
                .no_annotation()
        }));
        ListResourceTemplatesResult::with_all_items(resources)
    }

    fn list_prompts_result() -> ListPromptsResult {
        ListPromptsResult::with_all_items(vec![
            Prompt::new(
                PROMPT_STATUS_SUMMARY,
                Some("Summarize the current skill session status from resource-backed state."),
                Some(vec![prompt_session_argument()]),
            )
            .with_title("Skill session status summary"),
            Prompt::new(
                PROMPT_OWNER_GUIDANCE,
                Some("Summarize owner-facing next actions from canonical skill session resources."),
                Some(vec![prompt_session_argument()]),
            )
            .with_title("Skill owner guidance"),
            Prompt::new(
                PROMPT_PROPOSAL_PACKET,
                Some("Guide an agent through the owner-facing proposal packet using canonical session resources."),
                Some(vec![prompt_session_argument()]),
            )
            .with_title("Skill proposal packet"),
            Prompt::new(
                PROMPT_OPERATOR_GUIDANCE,
                Some("Summarize operator control, stop state, and failure evidence from canonical session resources."),
                Some(vec![prompt_session_argument()]),
            )
            .with_title("Skill operator guidance"),
            Prompt::new(
                PROMPT_CURRENT_STEP_GUIDANCE,
                Some("Summarize the current guided onboarding step from canonical install-backed resources."),
                Some(vec![prompt_install_argument()]),
            )
            .with_title("Onboarding current-step guidance"),
            Prompt::new(
                PROMPT_FAILURE_SUMMARY,
                Some("Summarize onboarding blockers, drift, and the last action rejection from canonical install-backed resources."),
                Some(vec![prompt_install_argument()]),
            )
            .with_title("Onboarding failure summary"),
            Prompt::new(
                PROMPT_ROUTE_READINESS_GUIDANCE,
                Some("Summarize canonical route readiness, where to inspect it, and the explicit owner action still required."),
                Some(vec![
                    prompt_install_argument(),
                    prompt_proposal_argument(),
                    prompt_route_argument(),
                ]),
            )
            .with_title("Route readiness guidance"),
            Prompt::new(
                PROMPT_ROUTE_BLOCKER_SUMMARY,
                Some("Summarize route-readiness blockers and incomplete evidence from canonical readiness resources."),
                Some(vec![
                    prompt_install_argument(),
                    prompt_proposal_argument(),
                    prompt_route_argument(),
                ]),
            )
            .with_title("Route blocker summary"),
            Prompt::new(
                PROMPT_RUNTIME_CONTROL_GUIDANCE,
                Some("Summarize canonical autonomous runtime control, where to inspect it, and the explicit recovery action required."),
                Some(vec![prompt_install_argument()]),
            )
            .with_title("Runtime control guidance"),
            Prompt::new(
                PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE,
                Some("Point agents at the canonical operator report and linked runtime-control resources for one approved selection."),
                Some(vec![
                    prompt_install_argument(),
                    prompt_proposal_argument(),
                    prompt_selection_argument(),
                ]),
            )
            .with_title("Strategy operator report guidance"),
            Prompt::new(
                PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE,
                Some("Point agents at the canonical recent-change report window and exception rollup for one approved selection."),
                Some(vec![
                    prompt_install_argument(),
                    prompt_proposal_argument(),
                    prompt_selection_argument(),
                ]),
            )
            .with_title("Strategy report window guidance"),
            Prompt::new(
                PROMPT_STRATEGY_SELECTION_GUIDANCE,
                Some("Summarize canonical strategy-selection state plus approved-runtime eligibility and monitoring resources."),
                Some(vec![
                    prompt_install_argument(),
                    prompt_proposal_argument(),
                    prompt_selection_argument(),
                ]),
            )
            .with_title("Strategy selection guidance"),
            Prompt::new(
                PROMPT_STRATEGY_SELECTION_DISCUSSION,
                Some("Summarize canonical recommendation basis, effective diff, and approval history for operator discussion after reconnect."),
                Some(vec![
                    prompt_install_argument(),
                    prompt_proposal_argument(),
                    prompt_selection_argument(),
                ]),
            )
            .with_title("Strategy selection discussion"),
            Prompt::new(
                PROMPT_STRATEGY_SELECTION_RECOVERY,
                Some("Summarize canonical diff, approval/reopen history, and live monitoring guidance for operator recovery after reconnect."),
                Some(vec![
                    prompt_install_argument(),
                    prompt_proposal_argument(),
                    prompt_selection_argument(),
                ]),
            )
            .with_title("Strategy selection recovery"),
        ])
    }
}

impl ServerHandler for A2exSkillMcpServer {
    fn get_info(&self) -> ServerInfo {
        Self::server_info()
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, RmcpError>> + Send + '_ {
        let result = Self::list_tools_result();
        // Strip $schema from inputSchema — OpenClaw cannot resolve the draft-2020-12 ref.
        // Tool is non-exhaustive, so we serialize → mutate → deserialize.
        let cleaned: Vec<serde_json::Value> = result.tools.iter().map(|tool| {
            let mut v = serde_json::to_value(tool).unwrap_or_default();
            if let Some(schema) = v.get_mut("inputSchema").and_then(|s| s.as_object_mut()) {
                schema.remove("$schema");
            }
            v
        }).collect();
        let cleaned_tools: Vec<Tool> = cleaned.into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        std::future::ready(Ok(ListToolsResult::with_all_items(cleaned_tools)))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, RmcpError>> + Send + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(Vec::new())))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, RmcpError>> + Send + '_ {
        std::future::ready(Ok(Self::list_resource_templates_result()))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, RmcpError>> + Send + '_ {
        let server = self.clone();
        async move {
            let (payload, uri) = if request.uri.starts_with("a2ex://skills/") {
                let (session_id, resource) =
                    parse_session_resource_uri(&request.uri).map_err(mcp_contract_to_rmcp_error)?;
                let payload = A2exSkillMcpServer::read_resource(
                    &server,
                    ReadSessionResourceRequest {
                        session_id,
                        resource,
                    },
                )
                .await
                .map_err(mcp_contract_to_rmcp_error)?;
                let uri =
                    resource.uri_for_session(payload["session_id"].as_str().unwrap_or_default());
                (payload, uri)
            } else if request.uri.starts_with("a2ex://onboarding/") {
                let (install_id, resource) = parse_onboarding_resource_uri(&request.uri)
                    .map_err(mcp_contract_to_rmcp_error)?;
                let inspection = server
                    .read_onboarding_inspection(&install_id)
                    .await
                    .map_err(mcp_contract_to_rmcp_error)?;
                let payload =
                    serde_json::to_value(onboarding_resource_payload(&inspection, resource))
                        .map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize onboarding resource payload: {error}"),
                                None,
                            )
                        })?;
                (payload, resource.uri_for_install(&install_id))
            } else if request.uri.starts_with("a2ex://runtime/") {
                let (install_id, resource) = parse_runtime_control_resource_uri(&request.uri)
                    .map_err(mcp_contract_to_rmcp_error)?;
                let record = server
                    .inspect_runtime_control(&install_id)
                    .await
                    .map_err(mcp_contract_to_rmcp_error)?;
                let payload = serde_json::to_value(runtime_control_resource_payload(
                    &install_id,
                    &record,
                    resource,
                ))
                .map_err(|error| {
                    RmcpError::internal_error(
                        format!("failed to serialize runtime control resource payload: {error}"),
                        None,
                    )
                })?;
                (payload, resource.uri_for_install(&install_id))
            } else if request.uri.starts_with("a2ex://strategy-runtime/") {
                let (install_id, proposal_id, selection_id, resource, cursor) =
                    parse_strategy_runtime_resource_uri(&request.uri)
                        .map_err(mcp_contract_to_rmcp_error)?;
                let selection_request = StrategySelectionReadRequest {
                    install_id: install_id.clone(),
                    proposal_id: proposal_id.clone(),
                    selection_id: selection_id.clone(),
                };
                let payload = match resource {
                    StrategyRuntimeResourceKind::Eligibility => {
                        serde_json::to_value(strategy_runtime_eligibility_resource_payload(
                            &server
                                .inspect_strategy_runtime_eligibility(selection_request.clone())
                                .await
                                .map_err(mcp_contract_to_rmcp_error)?,
                        ))
                    }
                    StrategyRuntimeResourceKind::Monitoring => {
                        serde_json::to_value(strategy_runtime_monitoring_resource_payload(
                            &server
                                .inspect_strategy_runtime_monitoring(selection_request.clone())
                                .await
                                .map_err(mcp_contract_to_rmcp_error)?,
                        ))
                    }
                    StrategyRuntimeResourceKind::OperatorReport => {
                        let report = server
                            .inspect_strategy_operator_report(selection_request.clone())
                            .await
                            .map_err(mcp_contract_to_rmcp_error)?;
                        serde_json::to_value(strategy_operator_report_resource_payload(
                            &report,
                            &selection_request,
                        ))
                    }
                    StrategyRuntimeResourceKind::ReportWindow => {
                        let report_window_cursor =
                            cursor.clone().expect("report-window cursor parsed");
                        let report_window = server
                            .inspect_strategy_report_window(
                                selection_request.clone(),
                                report_window_cursor.clone(),
                            )
                            .await
                            .map_err(mcp_contract_to_rmcp_error)?;
                        serde_json::to_value(strategy_report_window_resource_payload(
                            &report_window,
                            &selection_request,
                            &report_window_cursor,
                        ))
                    }
                    StrategyRuntimeResourceKind::ExceptionRollup => {
                        let exception_rollup = server
                            .inspect_strategy_exception_rollup(selection_request.clone())
                            .await
                            .map_err(mcp_contract_to_rmcp_error)?;
                        serde_json::to_value(strategy_exception_rollup_resource_payload(
                            &exception_rollup,
                            &selection_request,
                        ))
                    }
                }
                .map_err(|error| {
                    RmcpError::internal_error(
                        format!("failed to serialize strategy runtime resource payload: {error}"),
                        None,
                    )
                })?;
                let uri = match resource {
                    StrategyRuntimeResourceKind::ReportWindow => {
                        StrategyRuntimeResourceKind::report_window_uri_for_selection(
                            &install_id,
                            &proposal_id,
                            &selection_id,
                            cursor.as_deref().unwrap_or("bootstrap"),
                        )
                    }
                    _ => resource.uri_for_selection(&install_id, &proposal_id, &selection_id),
                };
                (payload, uri)
            } else if request.uri.starts_with("a2ex://strategy-selection/") {
                let (install_id, proposal_id, selection_id, resource) =
                    parse_strategy_selection_resource_uri(&request.uri)
                        .map_err(mcp_contract_to_rmcp_error)?;
                let inspection = server
                    .inspect_strategy_selection(StrategySelectionReadRequest {
                        install_id: install_id.clone(),
                        proposal_id: proposal_id.clone(),
                        selection_id: selection_id.clone(),
                    })
                    .await
                    .map_err(mcp_contract_to_rmcp_error)?;
                let payload = serde_json::to_value(strategy_selection_resource_payload(
                    &inspection,
                    resource,
                ))
                .map_err(|error| {
                    RmcpError::internal_error(
                        format!("failed to serialize strategy selection resource payload: {error}"),
                        None,
                    )
                })?;
                (
                    payload,
                    resource.uri_for_selection(&install_id, &proposal_id, &selection_id),
                )
            } else {
                let (install_id, proposal_id, route_id, resource) =
                    parse_route_readiness_resource_uri(&request.uri)
                        .map_err(mcp_contract_to_rmcp_error)?;
                let record = server
                    .read_route_readiness(ReadRouteReadinessRequest {
                        install_id: install_id.clone(),
                        proposal_id: proposal_id.clone(),
                        route_id: route_id.clone(),
                    })
                    .await
                    .map_err(mcp_contract_to_rmcp_error)?;
                let payload =
                    serde_json::to_value(route_readiness_resource_payload(&record, resource))
                        .map_err(|error| {
                            RmcpError::internal_error(
                                format!(
                                    "failed to serialize route readiness resource payload: {error}"
                                ),
                                None,
                            )
                        })?;
                (
                    payload,
                    resource.uri_for_route(&install_id, &proposal_id, &route_id),
                )
            };
            let text = serde_json::to_string_pretty(&payload).map_err(|error| {
                RmcpError::internal_error(
                    format!("failed to serialize MCP resource payload: {error}"),
                    None,
                )
            })?;
            Ok(ReadResourceResult::new(vec![
                ResourceContents::text(text, uri).with_mime_type("application/json"),
            ]))
        }
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, RmcpError>> + Send + '_ {
        let server = self.clone();
        async move {
            let prompt = match request.name.as_str() {
                PROMPT_CURRENT_STEP_GUIDANCE | PROMPT_FAILURE_SUMMARY => {
                    let install_id =
                        prompt_install_id_argument(&request).map_err(mcp_contract_to_rmcp_error)?;
                    let inspection = server
                        .read_onboarding_inspection(&install_id)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    render_onboarding_prompt(&request.name, &inspection)?
                }
                PROMPT_ROUTE_READINESS_GUIDANCE | PROMPT_ROUTE_BLOCKER_SUMMARY => {
                    let route_request = prompt_route_readiness_arguments(&request)
                        .map_err(mcp_contract_to_rmcp_error)?;
                    let record = server
                        .read_route_readiness(route_request.clone())
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    render_route_readiness_prompt(&request.name, &record, &route_request)?
                }
                PROMPT_RUNTIME_CONTROL_GUIDANCE => {
                    let install_id =
                        prompt_install_id_argument(&request).map_err(mcp_contract_to_rmcp_error)?;
                    let record = server
                        .inspect_runtime_control(&install_id)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    let current_selection = server
                        .latest_runtime_selection_request(&install_id)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    render_runtime_control_prompt(&install_id, &record, current_selection.as_ref())
                }
                PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE => {
                    let selection_request = prompt_strategy_selection_arguments(&request)
                        .map_err(mcp_contract_to_rmcp_error)?;
                    let report = server
                        .inspect_strategy_operator_report(selection_request.clone())
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    render_strategy_operator_report_prompt(&report, &selection_request)
                }
                PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE => {
                    let selection_request = prompt_strategy_selection_arguments(&request)
                        .map_err(mcp_contract_to_rmcp_error)?;
                    let report_window = server
                        .inspect_strategy_report_window(
                            selection_request.clone(),
                            "bootstrap".to_owned(),
                        )
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    let exception_rollup = server
                        .inspect_strategy_exception_rollup(selection_request.clone())
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    render_strategy_report_window_prompt(
                        &report_window,
                        &exception_rollup,
                        &selection_request,
                    )
                }
                PROMPT_STRATEGY_SELECTION_GUIDANCE
                | PROMPT_STRATEGY_SELECTION_DISCUSSION
                | PROMPT_STRATEGY_SELECTION_RECOVERY => {
                    let selection_request = prompt_strategy_selection_arguments(&request)
                        .map_err(mcp_contract_to_rmcp_error)?;
                    let inspection = server
                        .inspect_strategy_selection(selection_request.clone())
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    let runtime_eligibility = match server
                        .inspect_strategy_runtime_eligibility(selection_request.clone())
                        .await
                    {
                        Ok(eligibility) => Some(eligibility),
                        Err(McpContractError::StrategyRuntimeHandoff(
                            StrategyRuntimeHandoffError::NotFound { .. },
                        )) => None,
                        Err(error) => return Err(mcp_contract_to_rmcp_error(error)),
                    };
                    let runtime_monitoring = match server
                        .inspect_strategy_runtime_monitoring(selection_request.clone())
                        .await
                    {
                        Ok(monitoring) => Some(monitoring),
                        Err(McpContractError::StrategyRuntimeHandoff(
                            StrategyRuntimeHandoffError::NotFound { .. },
                        )) => None,
                        Err(error) => return Err(mcp_contract_to_rmcp_error(error)),
                    };
                    render_strategy_selection_prompt(
                        &request.name,
                        &inspection,
                        runtime_eligibility.as_ref(),
                        runtime_monitoring.as_ref(),
                        &selection_request,
                    )
                }
                _ => {
                    let session_id =
                        prompt_session_id_argument(&request).map_err(mcp_contract_to_rmcp_error)?;
                    A2exSkillMcpServer::render_prompt(
                        &server,
                        RenderPromptRequest {
                            session_id,
                            prompt_name: request.name,
                        },
                    )
                    .await
                    .map_err(mcp_contract_to_rmcp_error)?
                }
            };

            let mut messages = vec![PromptMessage::new_text(
                PromptMessageRole::User,
                prompt.content.clone(),
            )];
            messages.extend(prompt.referenced_resources.iter().map(|uri| {
                PromptMessage::new_resource_link(
                    PromptMessageRole::User,
                    RawResource::new(uri.clone(), resource_name_from_uri(uri))
                        .with_description("Referenced MCP resource")
                        .with_mime_type("application/json")
                        .no_annotation(),
                )
            }));

            Ok(GetPromptResult::new(messages).with_description(prompt.name))
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, RmcpError>> + Send + '_ {
        std::future::ready(Ok(Self::list_prompts_result()))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, RmcpError>> + Send + '_ {
        let server = self.clone();
        async move {
            let arguments = request.arguments.unwrap_or_default();
            let arguments = serde_json::Value::Object(arguments);
            match request.name.as_ref() {
                TOOL_LOAD_BUNDLE => {
                    let request = serde_json::from_value::<LoadBundleRequest>(arguments).map_err(
                        |error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_LOAD_BUNDLE} arguments: {error}"),
                                None,
                            )
                        },
                    )?;
                    let response = A2exSkillMcpServer::load_bundle(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_LOAD_BUNDLE} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_RELOAD_BUNDLE => {
                    let request = serde_json::from_value::<ReloadBundleRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_RELOAD_BUNDLE} arguments: {error}"),
                                None,
                            )
                        })?;
                    let response = A2exSkillMcpServer::reload_bundle(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!(
                                    "failed to serialize {TOOL_RELOAD_BUNDLE} response: {error}"
                                ),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_GENERATE_PROPOSAL_PACKET => {
                    let request =
                        serde_json::from_value::<GenerateProposalPacketRequest>(arguments)
                            .map_err(|error| {
                                RmcpError::invalid_params(
                                    format!(
                                        "invalid {TOOL_GENERATE_PROPOSAL_PACKET} arguments: {error}"
                                    ),
                                    None,
                                )
                            })?;
                    let response = A2exSkillMcpServer::generate_proposal_packet(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!(
                                    "failed to serialize {TOOL_GENERATE_PROPOSAL_PACKET} response: {error}"
                                ),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_STOP_SESSION => {
                    let request = serde_json::from_value::<StopSessionRequest>(arguments).map_err(
                        |error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_STOP_SESSION} arguments: {error}"),
                                None,
                            )
                        },
                    )?;
                    let response = A2exSkillMcpServer::stop_session(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!(
                                    "failed to serialize {TOOL_STOP_SESSION} response: {error}"
                                ),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_CLEAR_STOP => {
                    let request =
                        serde_json::from_value::<ClearStopRequest>(arguments).map_err(|error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_CLEAR_STOP} arguments: {error}"),
                                None,
                            )
                        })?;
                    let response = A2exSkillMcpServer::clear_stop(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_CLEAR_STOP} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_RUNTIME_STOP => {
                    let request =
                        serde_json::from_value::<RuntimeControlMutationRequest>(arguments)
                            .map_err(|error| {
                                RmcpError::invalid_params(
                                    format!("invalid {TOOL_RUNTIME_STOP} arguments: {error}"),
                                    None,
                                )
                            })?;
                    let response = A2exSkillMcpServer::stop_runtime(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!(
                                    "failed to serialize {TOOL_RUNTIME_STOP} response: {error}"
                                ),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_RUNTIME_PAUSE => {
                    let request =
                        serde_json::from_value::<RuntimeControlMutationRequest>(arguments)
                            .map_err(|error| {
                                RmcpError::invalid_params(
                                    format!("invalid {TOOL_RUNTIME_PAUSE} arguments: {error}"),
                                    None,
                                )
                            })?;
                    let response = A2exSkillMcpServer::pause_runtime(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!(
                                    "failed to serialize {TOOL_RUNTIME_PAUSE} response: {error}"
                                ),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_RUNTIME_CLEAR_STOP => {
                    let request =
                        serde_json::from_value::<RuntimeControlMutationRequest>(arguments)
                            .map_err(|error| {
                                RmcpError::invalid_params(
                                    format!("invalid {TOOL_RUNTIME_CLEAR_STOP} arguments: {error}"),
                                    None,
                                )
                            })?;
                    let response = A2exSkillMcpServer::clear_runtime_stop(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(serde_json::to_value(response).map_err(
                        |error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_RUNTIME_CLEAR_STOP} response: {error}"),
                                None,
                            )
                        },
                    )?))
                }
                TOOL_BOOTSTRAP_INSTALL => {
                    let request =
                        serde_json::from_value::<OnboardingBootstrapInstallRequest>(arguments)
                            .map_err(|error| {
                                RmcpError::invalid_params(
                                    format!("invalid {TOOL_BOOTSTRAP_INSTALL} arguments: {error}"),
                                    None,
                                )
                            })?;
                    let response = A2exSkillMcpServer::bootstrap_install(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!(
                                    "failed to serialize {TOOL_BOOTSTRAP_INSTALL} response: {error}"
                                ),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_APPLY_ONBOARDING_ACTION => {
                    let request = serde_json::from_value::<OnboardingApplyActionRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!(
                                    "invalid {TOOL_APPLY_ONBOARDING_ACTION} arguments: {error}"
                                ),
                                None,
                            )
                        })?;
                    let response = A2exSkillMcpServer::apply_onboarding_action(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_APPLY_ONBOARDING_ACTION} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_EVALUATE_ROUTE_READINESS => {
                    let request =
                        serde_json::from_value::<EvaluateRouteReadinessRequest>(arguments)
                            .map_err(|error| {
                                RmcpError::invalid_params(
                                    format!(
                                        "invalid {TOOL_EVALUATE_ROUTE_READINESS} arguments: {error}"
                                    ),
                                    None,
                                )
                            })?;
                    let response = A2exSkillMcpServer::evaluate_route_readiness(&server, request)
                        .await
                        .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_EVALUATE_ROUTE_READINESS} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_APPLY_ROUTE_READINESS_ACTION => {
                    let request = serde_json::from_value::<ApplyRouteReadinessActionRequest>(
                        arguments,
                    )
                    .map_err(|error| {
                        RmcpError::invalid_params(
                            format!(
                                "invalid {TOOL_APPLY_ROUTE_READINESS_ACTION} arguments: {error}"
                            ),
                            None,
                        )
                    })?;
                    let response =
                        A2exSkillMcpServer::apply_route_readiness_action(&server, request)
                            .await
                            .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_APPLY_ROUTE_READINESS_ACTION} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_STRATEGY_SELECTION_MATERIALIZE => {
                    let request = serde_json::from_value::<StrategySelectionMaterializeRequest>(
                        arguments,
                    )
                    .map_err(|error| {
                        RmcpError::invalid_params(
                            format!(
                                "invalid {TOOL_STRATEGY_SELECTION_MATERIALIZE} arguments: {error}"
                            ),
                            None,
                        )
                    })?;
                    let response =
                        A2exSkillMcpServer::materialize_strategy_selection_record(&server, request)
                            .await
                            .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_STRATEGY_SELECTION_MATERIALIZE} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE => {
                    let request = serde_json::from_value::<StrategySelectionApplyOverrideRequest>(
                        arguments,
                    )
                    .map_err(|error| {
                        RmcpError::invalid_params(
                            format!(
                                "invalid {TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE} arguments: {error}"
                            ),
                            None,
                        )
                    })?;
                    let response = A2exSkillMcpServer::apply_strategy_selection_override_record(
                        &server, request,
                    )
                    .await
                    .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_STRATEGY_SELECTION_APPROVE => {
                    let request = serde_json::from_value::<StrategySelectionApproveRequest>(
                        arguments,
                    )
                    .map_err(|error| {
                        RmcpError::invalid_params(
                            format!("invalid {TOOL_STRATEGY_SELECTION_APPROVE} arguments: {error}"),
                            None,
                        )
                    })?;
                    let response =
                        A2exSkillMcpServer::approve_strategy_selection_record(&server, request)
                            .await
                            .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_STRATEGY_SELECTION_APPROVE} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                TOOL_STRATEGY_SELECTION_REOPEN => {
                    let request = serde_json::from_value::<StrategySelectionReopenRequest>(
                        arguments,
                    )
                    .map_err(|error| {
                        RmcpError::invalid_params(
                            format!("invalid {TOOL_STRATEGY_SELECTION_REOPEN} arguments: {error}"),
                            None,
                        )
                    })?;
                    let response =
                        A2exSkillMcpServer::reopen_strategy_selection_record(&server, request)
                            .await
                            .map_err(mcp_contract_to_rmcp_error)?;
                    Ok(CallToolResult::structured(
                        serde_json::to_value(response).map_err(|error| {
                            RmcpError::internal_error(
                                format!("failed to serialize {TOOL_STRATEGY_SELECTION_REOPEN} response: {error}"),
                                None,
                            )
                        })?,
                    ))
                }
                // ----- venue.* tool dispatch -----
                TOOL_VENUE_PREPARE_BRIDGE => {
                    let request = serde_json::from_value::<PrepareBridgeRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_VENUE_PREPARE_BRIDGE} arguments: {error}"),
                                None,
                            )
                        })?;
                    match server.handle_prepare_bridge(request).await {
                        Ok(response) => Ok(CallToolResult::structured(
                            serde_json::to_value(response).map_err(|error| {
                                RmcpError::internal_error(
                                    format!("failed to serialize {TOOL_VENUE_PREPARE_BRIDGE} response: {error}"),
                                    None,
                                )
                            })?,
                        )),
                        Err(e) => Ok(CallToolResult::error(vec![
                            rmcp::model::Content::text(format!("venue error (prepare_bridge): {e}")),
                        ])),
                    }
                }
                TOOL_VENUE_TRADE_HYPERLIQUID => {
                    let request = serde_json::from_value::<TradeHyperliquidRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!(
                                    "invalid {TOOL_VENUE_TRADE_HYPERLIQUID} arguments: {error}"
                                ),
                                None,
                            )
                        })?;
                    match server.handle_trade_hyperliquid(request).await {
                        Ok(response) => Ok(CallToolResult::structured(
                            serde_json::to_value(response).map_err(|error| {
                                RmcpError::internal_error(
                                    format!("failed to serialize {TOOL_VENUE_TRADE_HYPERLIQUID} response: {error}"),
                                    None,
                                )
                            })?,
                        )),
                        Err(e) => Ok(CallToolResult::error(vec![
                            rmcp::model::Content::text(format!("venue error (hyperliquid): {e}")),
                        ])),
                    }
                }
                TOOL_VENUE_TRADE_POLYMARKET => {
                    let request = serde_json::from_value::<TradePolymarketRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_VENUE_TRADE_POLYMARKET} arguments: {error}"),
                                None,
                            )
                        })?;
                    match server.handle_trade_polymarket(request).await {
                        Ok(response) => Ok(CallToolResult::structured(
                            serde_json::to_value(response).map_err(|error| {
                                RmcpError::internal_error(
                                    format!("failed to serialize {TOOL_VENUE_TRADE_POLYMARKET} response: {error}"),
                                    None,
                                )
                            })?,
                        )),
                        Err(e) => Ok(CallToolResult::error(vec![
                            rmcp::model::Content::text(format!("venue error (polymarket): {e}")),
                        ])),
                    }
                }
                TOOL_VENUE_DERIVE_API_KEY => {
                    let request = serde_json::from_value::<DeriveApiKeyRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_VENUE_DERIVE_API_KEY} arguments: {error}"),
                                None,
                            )
                        })?;
                    match server.handle_derive_api_key(request).await {
                        Ok(response) => Ok(CallToolResult::structured(
                            serde_json::to_value(response).map_err(|error| {
                                RmcpError::internal_error(
                                    format!("failed to serialize {TOOL_VENUE_DERIVE_API_KEY} response: {error}"),
                                    None,
                                )
                            })?,
                        )),
                        Err(e) => Ok(CallToolResult::error(vec![
                            rmcp::model::Content::text(format!("venue error (derive_api_key): {e}")),
                        ])),
                    }
                }
                TOOL_VENUE_QUERY_POSITIONS => {
                    let request = serde_json::from_value::<QueryPositionsRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_VENUE_QUERY_POSITIONS} arguments: {error}"),
                                None,
                            )
                        })?;
                    match server.handle_query_positions(request).await {
                        Ok(response) => Ok(CallToolResult::structured(
                            serde_json::to_value(response).map_err(|error| {
                                RmcpError::internal_error(
                                    format!("failed to serialize {TOOL_VENUE_QUERY_POSITIONS} response: {error}"),
                                    None,
                                )
                            })?,
                        )),
                        Err(e) => Ok(CallToolResult::error(vec![
                            rmcp::model::Content::text(format!("venue error (query_positions): {e}")),
                        ])),
                    }
                }
                TOOL_VENUE_BRIDGE_STATUS => {
                    let request = serde_json::from_value::<BridgeStatusRequest>(arguments)
                        .map_err(|error| {
                            RmcpError::invalid_params(
                                format!("invalid {TOOL_VENUE_BRIDGE_STATUS} arguments: {error}"),
                                None,
                            )
                        })?;
                    match server.handle_bridge_status(request).await {
                        Ok(response) => Ok(CallToolResult::structured(
                            serde_json::to_value(response).map_err(|error| {
                                RmcpError::internal_error(
                                    format!("failed to serialize {TOOL_VENUE_BRIDGE_STATUS} response: {error}"),
                                    None,
                                )
                            })?,
                        )),
                        Err(e) => Ok(CallToolResult::error(vec![
                            rmcp::model::Content::text(format!("venue error (bridge_status): {e}")),
                        ])),
                    }
                }
                // ----- Layer 3: Venue Recipes -----
                TOOL_POLYMARKET_TRADE => {
                    let req = serde_json::from_value::<PolymarketTradeRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid polymarket.trade args: {e}"), None))?;
                    match server.handle_polymarket_trade(req).await {
                        Ok(r) => {
                            let val = serde_json::to_value(&r).unwrap_or_default();
                            if r.error.is_some() {
                                Ok(CallToolResult::structured_error(val))
                            } else {
                                Ok(CallToolResult::structured(val))
                            }
                        }
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                TOOL_HYPERLIQUID_TRADE => {
                    let req = serde_json::from_value::<HyperliquidTradeRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid hyperliquid.trade args: {e}"), None))?;
                    match server.handle_hyperliquid_trade(req).await {
                        Ok(r) => {
                            let val = serde_json::to_value(&r).unwrap_or_default();
                            if r.error.is_some() {
                                Ok(CallToolResult::structured_error(val))
                            } else {
                                Ok(CallToolResult::structured(val))
                            }
                        }
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                // ----- Layer 2: DeFi Primitives -----
                TOOL_DEFI_BRIDGE => {
                    let req = serde_json::from_value::<DefiBridgeRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid defi.bridge args: {e}"), None))?;
                    match server.handle_defi_bridge(req).await {
                        Ok(r) => {
                            let val = serde_json::to_value(&r).unwrap_or_default();
                            if r.status == "filled" {
                                Ok(CallToolResult::structured(val))
                            } else {
                                Ok(CallToolResult::structured_error(val))
                            }
                        }
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                TOOL_DEFI_APPROVE => {
                    let req = serde_json::from_value::<DefiApproveRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid defi.approve args: {e}"), None))?;
                    match server.handle_defi_approve(req).await {
                        Ok(r) => {
                            let val = serde_json::to_value(&r).unwrap_or_default();
                            if r.status == "confirmed" {
                                Ok(CallToolResult::structured(val))
                            } else {
                                Ok(CallToolResult::structured_error(val))
                            }
                        }
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                TOOL_DEFI_ANALYZE => {
                    let req = serde_json::from_value::<DefiAnalyzeRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid defi.analyze args: {e}"), None))?;
                    match server.handle_defi_analyze(req).await {
                        Ok(r) => Ok(CallToolResult::structured(serde_json::to_value(r).unwrap_or_default())),
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                // ----- Layer 1: Chain Primitives -----
                TOOL_CHAIN_READ => {
                    let req = serde_json::from_value::<ChainReadRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid chain.read args: {e}"), None))?;
                    match server.handle_chain_read(req).await {
                        Ok(r) => Ok(CallToolResult::structured(serde_json::to_value(r).unwrap_or_default())),
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                TOOL_CHAIN_EXECUTE => {
                    let req = serde_json::from_value::<ChainExecuteRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid chain.execute args: {e}"), None))?;
                    match server.handle_chain_execute(req).await {
                        Ok(r) => {
                            let is_err = r.status != "confirmed";
                            let val = serde_json::to_value(&r).unwrap_or_default();
                            if is_err {
                                Ok(CallToolResult::structured_error(val))
                            } else {
                                Ok(CallToolResult::structured(val))
                            }
                        }
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                TOOL_CHAIN_BALANCE => {
                    let req = serde_json::from_value::<ChainBalanceRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid chain.balance args: {e}"), None))?;
                    match server.handle_chain_balance(req).await {
                        Ok(r) => Ok(CallToolResult::structured(serde_json::to_value(r).unwrap_or_default())),
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                TOOL_CHAIN_SIMULATE => {
                    let req = serde_json::from_value::<ChainSimulateRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid chain.simulate args: {e}"), None))?;
                    match server.handle_chain_simulate(req).await {
                        Ok(r) => Ok(CallToolResult::structured(serde_json::to_value(r).unwrap_or_default())),
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(e)])),
                    }
                }
                "hyperliquid.withdraw" => {
                    let amount = arguments.get("amount").and_then(|v| v.as_str()).unwrap_or("0").to_string();
                    let destination = arguments.get("destination").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if destination.is_empty() {
                        return Ok(CallToolResult::error(vec![rmcp::model::Content::text("destination address required")]));
                    }
                    let adapters = server.get_venue_adapters().map_err(|e| RmcpError::internal_error(e.to_string(), None))?;
                    let wallet_addr = server.resolve_hot_wallet_address().unwrap_or_default();
                    match adapters.hyperliquid.transport().withdraw(&amount, &destination, &wallet_addr).await {
                        Ok(resp) => Ok(CallToolResult::structured(serde_json::json!({"status": "ok", "response": resp}))),
                        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(format!("withdraw failed: {e}"))])),
                    }
                }
                "session.refresh" => {
                    let req = serde_json::from_value::<SessionRefreshRequest>(arguments)
                        .map_err(|e| RmcpError::invalid_params(format!("invalid session.refresh args: {e}"), None))?;
                    // SAFETY: set_var is safe here because a2ex-mcp is single-threaded MCP stdio server.
                    // The token is consumed by handle_chain_execute which reads env var each call.
                    unsafe { std::env::set_var("A2EX_HOT_SESSION_TOKEN", &req.session_token); }
                    tracing::info!("session token refreshed via MCP tool");
                    Ok(CallToolResult::structured(serde_json::json!({"status": "refreshed"})))
                }
                _ => Err(RmcpError::invalid_params(
                    format!("unknown MCP tool {}", request.name),
                    None,
                )),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillSurfaceContract {
    pub server_name: String,
    pub tools: Vec<McpToolDescriptor>,
    pub resources: Vec<McpResourceDescriptor>,
    pub prompts: Vec<McpPromptDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpResourceDescriptor {
    pub uri_template: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpPromptDescriptor {
    pub name: String,
    pub description: String,
    pub resource_templates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OnboardingBootstrapInstallRequest {
    pub install_url: String,
    pub workspace_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_install_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingBootstrapInstallResponse {
    pub workspace_id: String,
    pub install_id: String,
    pub claim_disposition: String,
    pub attached_bundle_url: String,
    pub aggregate_status: OnboardingAggregateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_handoff: Option<ProposalHandoff>,
    pub guided_state_uri: String,
    pub checklist_uri: String,
    pub diagnostics_uri: String,
    pub prompt_names: Vec<String>,
}

impl OnboardingBootstrapInstallResponse {
    fn from_parts(
        result: a2ex_onboarding::InstallBootstrapResult,
        inspection: GuidedOnboardingInspection,
    ) -> Self {
        let install_id = result.install_id.clone();
        Self {
            workspace_id: result.workspace_id,
            install_id: install_id.clone(),
            claim_disposition: result.claim_disposition.as_str().to_owned(),
            attached_bundle_url: result.attached_bundle_url.to_string(),
            aggregate_status: inspection.aggregate_status,
            current_step_key: inspection.current_step_key,
            recommended_action: inspection.recommended_action,
            proposal_handoff: result.proposal_handoff,
            guided_state_uri: OnboardingResourceKind::GuidedState.uri_for_install(&install_id),
            checklist_uri: OnboardingResourceKind::Checklist.uri_for_install(&install_id),
            diagnostics_uri: OnboardingResourceKind::Diagnostics.uri_for_install(&install_id),
            prompt_names: vec![
                PROMPT_CURRENT_STEP_GUIDANCE.to_owned(),
                PROMPT_FAILURE_SUMMARY.to_owned(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OnboardingApplyActionRequest {
    pub install_id: String,
    pub action: GuidedOnboardingAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingApplyActionResponse {
    pub install_id: String,
    pub aggregate_status: OnboardingAggregateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
}

impl From<a2ex_onboarding::GuidedOnboardingActionResult> for OnboardingApplyActionResponse {
    fn from(value: a2ex_onboarding::GuidedOnboardingActionResult) -> Self {
        Self {
            install_id: value.install_id,
            aggregate_status: value.aggregate_status,
            current_step_key: value.current_step_key,
            recommended_action: value.recommended_action,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvaluateRouteReadinessRequest {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadRouteReadinessRequest {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeControlMutationRequest {
    pub install_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeControlMutationResponse {
    pub install_id: String,
    pub scope_key: String,
    pub command: String,
    pub control_mode: String,
    pub autonomy_eligibility: String,
    pub transition_reason: String,
    pub transition_source: String,
    pub transitioned_at: String,
    pub status_uri: String,
    pub failures_uri: String,
}

impl RuntimeControlMutationResponse {
    fn from_record(install_id: &str, command: &str, record: PersistedRuntimeControl) -> Self {
        Self {
            install_id: install_id.to_owned(),
            scope_key: record.scope_key.clone(),
            command: command.to_owned(),
            control_mode: record.control_mode.clone(),
            autonomy_eligibility: runtime_control_autonomy_eligibility(&record.control_mode)
                .to_owned(),
            transition_reason: record.transition_reason.clone(),
            transition_source: record.transition_source.clone(),
            transitioned_at: record.transitioned_at.clone(),
            status_uri: RuntimeControlResourceKind::Status.uri_for_install(install_id),
            failures_uri: RuntimeControlResourceKind::Failures.uri_for_install(install_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApplyRouteReadinessActionRequest {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub action: RouteReadinessAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyRouteReadinessActionResponse {
    pub install_id: String,
    pub proposal_id: String,
    pub route_id: String,
    pub status: a2ex_onboarding::RouteReadinessStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<RouteReadinessActionRef>,
    pub stale: a2ex_onboarding::RouteReadinessStaleState,
}

impl From<RouteReadinessActionResult> for ApplyRouteReadinessActionResponse {
    fn from(value: RouteReadinessActionResult) -> Self {
        Self {
            install_id: value.install_id,
            proposal_id: value.proposal_id,
            route_id: value.route_id,
            status: value.status,
            current_step_key: value.current_step_key,
            recommended_action: value.recommended_action,
            stale: value.stale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessSummaryResource {
    #[serde(flatten)]
    pub record: RouteReadinessRecord,
    pub summary_uri: String,
    pub progress_uri: String,
    pub blockers_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessProgressResource {
    pub identity: a2ex_onboarding::RouteReadinessIdentity,
    pub status: a2ex_onboarding::RouteReadinessStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordered_steps: Vec<a2ex_onboarding::RouteReadinessStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<RouteReadinessActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<a2ex_onboarding::RouteReadinessStaleState>,
    #[serde(default)]
    pub last_rejection: Option<a2ex_onboarding::RouteReadinessActionRejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<a2ex_onboarding::RouteReadinessEvaluationMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluated_at: Option<String>,
    pub summary_uri: String,
    pub progress_uri: String,
    pub blockers_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReadinessBlockersResource {
    pub identity: a2ex_onboarding::RouteReadinessIdentity,
    pub status: a2ex_onboarding::RouteReadinessStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<a2ex_onboarding::RouteReadinessBlocker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_owner_action: Option<a2ex_onboarding::RouteOwnerAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<a2ex_onboarding::RouteReadinessStaleState>,
    #[serde(default)]
    pub last_rejection: Option<a2ex_onboarding::RouteReadinessActionRejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<a2ex_onboarding::RouteReadinessEvaluationMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluated_at: Option<String>,
    pub summary_uri: String,
    pub progress_uri: String,
    pub blockers_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionMaterializeRequest {
    pub install_id: String,
    pub proposal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionOverrideInput {
    pub key: String,
    pub value: serde_json::Value,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionApplyOverrideRequest {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    #[serde(rename = "override")]
    pub r#override: StrategySelectionOverrideInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionApprovalPayload {
    pub approved_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionApproveRequest {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub expected_selection_revision: u32,
    pub approval: StrategySelectionApprovalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionReopenRequest {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionReadRequest {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionMutationSummary {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionMutationResponse {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
    pub summary: StrategySelectionMutationSummary,
    pub summary_uri: String,
    pub overrides_uri: String,
    pub approval_uri: String,
    pub diff_uri: String,
    pub approval_history_uri: String,
    pub eligibility_uri: String,
    pub monitoring_uri: String,
}

impl StrategySelectionMutationResponse {
    fn from_record(record: &StrategySelectionRecord) -> Self {
        Self {
            install_id: record.install_id.clone(),
            proposal_id: record.proposal_id.clone(),
            selection_id: record.selection_id.clone(),
            selection_revision: record.selection_revision,
            status: record.status.as_str().to_owned(),
            summary: StrategySelectionMutationSummary {
                install_id: record.install_id.clone(),
                proposal_id: record.proposal_id.clone(),
                selection_id: record.selection_id.clone(),
                selection_revision: record.selection_revision,
                status: record.status.as_str().to_owned(),
            },
            summary_uri: StrategySelectionResourceKind::Summary.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            overrides_uri: StrategySelectionResourceKind::Overrides.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            approval_uri: StrategySelectionResourceKind::Approval.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            diff_uri: StrategySelectionResourceKind::Diff.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            approval_history_uri: StrategySelectionResourceKind::ApprovalHistory.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            eligibility_uri: StrategyRuntimeResourceKind::Eligibility.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            monitoring_uri: StrategyRuntimeResourceKind::Monitoring.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionSummaryResource {
    #[serde(flatten)]
    pub summary: StrategySelectionRecord,
    pub summary_uri: String,
    pub overrides_uri: String,
    pub approval_uri: String,
    pub diff_uri: String,
    pub approval_history_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionOverridesResource {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
    #[serde(default)]
    pub overrides: Vec<StrategySelectionOverrideRecord>,
    pub summary_uri: String,
    pub overrides_uri: String,
    pub approval_uri: String,
    pub diff_uri: String,
    pub approval_history_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionApprovalResource {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
    pub approved_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    pub summary_uri: String,
    pub overrides_uri: String,
    pub approval_uri: String,
    pub diff_uri: String,
    pub approval_history_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionDiffResource {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
    pub baseline_kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_override_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_sensitive_changes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisory_changes: Vec<String>,
    pub readiness_stale: bool,
    pub approval_stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_stale_reason: Option<String>,
    pub summary_uri: String,
    pub overrides_uri: String,
    pub approval_uri: String,
    pub diff_uri: String,
    pub approval_history_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySelectionApprovalHistoryResource {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
    #[serde(default)]
    pub events: Vec<a2ex_onboarding::StrategySelectionApprovalHistoryEvent>,
    pub summary_uri: String,
    pub overrides_uri: String,
    pub approval_uri: String,
    pub diff_uri: String,
    pub approval_history_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StrategySelectionReopenResponse {
    pub install_id: String,
    pub proposal_id: String,
    pub selection_id: String,
    pub selection_revision: u32,
    pub status: String,
    pub diff_uri: String,
    pub approval_history_uri: String,
    pub monitoring_uri: String,
}

impl StrategySelectionReopenResponse {
    fn from_record(record: &StrategySelectionRecord) -> Self {
        Self {
            install_id: record.install_id.clone(),
            proposal_id: record.proposal_id.clone(),
            selection_id: record.selection_id.clone(),
            selection_revision: record.selection_revision,
            status: record.status.as_str().to_owned(),
            diff_uri: StrategySelectionResourceKind::Diff.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            approval_history_uri: StrategySelectionResourceKind::ApprovalHistory.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
            monitoring_uri: StrategyRuntimeResourceKind::Monitoring.uri_for_selection(
                &record.install_id,
                &record.proposal_id,
                &record.selection_id,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySelectionResourceKind {
    Summary,
    Overrides,
    Approval,
    Diff,
    ApprovalHistory,
}

impl StrategySelectionResourceKind {
    pub fn all() -> [Self; 5] {
        [
            Self::Summary,
            Self::Overrides,
            Self::Approval,
            Self::Diff,
            Self::ApprovalHistory,
        ]
    }

    pub fn as_path_segment(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Overrides => "overrides",
            Self::Approval => "approval",
            Self::Diff => "diff",
            Self::ApprovalHistory => "approval-history",
        }
    }

    pub fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "summary" => Some(Self::Summary),
            "overrides" => Some(Self::Overrides),
            "approval" => Some(Self::Approval),
            "diff" => Some(Self::Diff),
            "approval-history" => Some(Self::ApprovalHistory),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Summary => "strategy selection summary",
            Self::Overrides => "strategy selection overrides",
            Self::Approval => "strategy selection approval",
            Self::Diff => "strategy selection diff",
            Self::ApprovalHistory => "strategy selection approval history",
        }
    }

    pub fn template(self) -> String {
        format!(
            "a2ex://strategy-selection/selections/{{install_id}}/{{proposal_id}}/{{selection_id}}/{}",
            self.as_path_segment()
        )
    }

    pub fn uri_for_selection(
        self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> String {
        format!(
            "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/{}",
            self.as_path_segment()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeEligibilityResource {
    #[serde(flatten)]
    pub handoff: StrategyRuntimeHandoffRecord,
    pub eligibility_uri: String,
    pub monitoring_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRuntimeMonitoringResource {
    #[serde(flatten)]
    pub monitoring: StrategyRuntimeMonitoringSummary,
    pub eligibility_uri: String,
    pub monitoring_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyOperatorReportResource {
    #[serde(flatten)]
    pub report: StrategyOperatorReport,
    pub operator_report_uri: String,
    pub eligibility_uri: String,
    pub monitoring_uri: String,
    pub runtime_control_status_uri: String,
    pub runtime_control_failures_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyReportWindowResource {
    #[serde(flatten)]
    pub report_window: StrategyReportWindow,
    pub report_window_uri: String,
    pub exception_rollup_uri: String,
    pub operator_report_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyExceptionRollupResource {
    pub report_kind: String,
    pub identity: a2ex_onboarding::StrategyReportWindowIdentity,
    pub owner_action_needed_now: bool,
    pub urgency: a2ex_onboarding::StrategyExceptionUrgency,
    pub recommended_operator_action: String,
    pub active_hold: Option<a2ex_onboarding::StrategyRuntimeHoldException>,
    pub last_runtime_failure: Option<a2ex_onboarding::StrategyRuntimeLastOutcome>,
    pub last_runtime_rejection: Option<a2ex_onboarding::StrategyRuntimeLastOutcome>,
    pub exception_rollup_uri: String,
    pub bootstrap_report_window_uri: String,
    pub operator_report_uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyRuntimeResourceKind {
    Eligibility,
    Monitoring,
    OperatorReport,
    ReportWindow,
    ExceptionRollup,
}

impl StrategyRuntimeResourceKind {
    pub fn all() -> [Self; 5] {
        [
            Self::Eligibility,
            Self::Monitoring,
            Self::OperatorReport,
            Self::ReportWindow,
            Self::ExceptionRollup,
        ]
    }

    pub fn as_path_segment(self) -> &'static str {
        match self {
            Self::Eligibility => "eligibility",
            Self::Monitoring => "monitoring",
            Self::OperatorReport => "operator-report",
            Self::ReportWindow => "report-window",
            Self::ExceptionRollup => "exception-rollup",
        }
    }

    pub fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "eligibility" => Some(Self::Eligibility),
            "monitoring" => Some(Self::Monitoring),
            "operator-report" => Some(Self::OperatorReport),
            "report-window" => Some(Self::ReportWindow),
            "exception-rollup" => Some(Self::ExceptionRollup),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Eligibility => "strategy runtime eligibility",
            Self::Monitoring => "strategy runtime monitoring",
            Self::OperatorReport => "strategy runtime operator report",
            Self::ReportWindow => "strategy runtime report window",
            Self::ExceptionRollup => "strategy runtime exception rollup",
        }
    }

    pub fn template(self) -> String {
        match self {
            Self::ReportWindow => "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/report-window/{cursor}".to_owned(),
            _ => format!(
                "a2ex://strategy-runtime/selections/{{install_id}}/{{proposal_id}}/{{selection_id}}/{}",
                self.as_path_segment()
            ),
        }
    }

    pub fn uri_for_selection(
        self,
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
    ) -> String {
        debug_assert!(self != Self::ReportWindow);
        format!(
            "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/{}",
            self.as_path_segment()
        )
    }

    pub fn report_window_uri_for_selection(
        install_id: &str,
        proposal_id: &str,
        selection_id: &str,
        cursor: &str,
    ) -> String {
        let encoded_cursor =
            url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect::<String>();
        format!(
            "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/report-window/{encoded_cursor}"
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeControlStatusResource {
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeControlRejection {
    pub code: String,
    pub message: String,
    pub attempted_operation: String,
    pub rejected_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeControlFailuresResource {
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
    pub last_rejection: Option<RuntimeControlRejection>,
    pub updated_at: String,
    pub status_uri: String,
    pub failures_uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeControlResourceKind {
    Status,
    Failures,
}

impl RuntimeControlResourceKind {
    pub fn all() -> [Self; 2] {
        [Self::Status, Self::Failures]
    }

    pub fn as_path_segment(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Failures => "failures",
        }
    }

    pub fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "status" => Some(Self::Status),
            "failures" => Some(Self::Failures),
            _ => None,
        }
    }

    pub fn template(self) -> String {
        format!(
            "a2ex://runtime/control/{{install_id}}/{}",
            self.as_path_segment()
        )
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Status => "runtime control status",
            Self::Failures => "runtime control failures",
        }
    }

    pub fn uri_for_install(self, install_id: &str) -> String {
        format!(
            "a2ex://runtime/control/{install_id}/{}",
            self.as_path_segment()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingGuidedStateResource {
    pub install_id: String,
    pub workspace_id: String,
    pub attached_bundle_url: String,
    pub aggregate_status: OnboardingAggregateStatus,
    pub ordered_steps: Vec<GuidedOnboardingStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_handoff: Option<ProposalHandoff>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingChecklistResource {
    pub install_id: String,
    pub workspace_id: String,
    pub attached_bundle_url: String,
    pub aggregate_status: OnboardingAggregateStatus,
    pub checklist_items: Vec<OnboardingChecklistItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingDiagnosticsResource {
    pub install_id: String,
    pub workspace_id: String,
    pub attached_bundle_url: String,
    pub bootstrap: BootstrapReport,
    pub aggregate_status: OnboardingAggregateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_action: Option<GuidedOnboardingActionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_handoff: Option<ProposalHandoff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<OnboardingBundleDrift>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejection: Option<a2ex_onboarding::GuidedOnboardingActionRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingResourceKind {
    GuidedState,
    Checklist,
    Diagnostics,
}

impl OnboardingResourceKind {
    pub fn all() -> [Self; 3] {
        [Self::GuidedState, Self::Checklist, Self::Diagnostics]
    }

    pub fn as_path_segment(self) -> &'static str {
        match self {
            Self::GuidedState => "guided_state",
            Self::Checklist => "checklist",
            Self::Diagnostics => "diagnostics",
        }
    }

    pub fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "guided_state" => Some(Self::GuidedState),
            "checklist" => Some(Self::Checklist),
            "diagnostics" => Some(Self::Diagnostics),
            _ => None,
        }
    }

    pub fn template(self) -> String {
        format!(
            "a2ex://onboarding/installs/{{install_id}}/{}",
            self.as_path_segment()
        )
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::GuidedState => "guided onboarding state",
            Self::Checklist => "guided onboarding checklist",
            Self::Diagnostics => "guided onboarding diagnostics",
        }
    }

    pub fn uri_for_install(self, install_id: &str) -> String {
        format!(
            "a2ex://onboarding/installs/{install_id}/{}",
            self.as_path_segment()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReadinessResourceKind {
    Summary,
    Progress,
    Blockers,
}

impl RouteReadinessResourceKind {
    pub fn all() -> [Self; 3] {
        [Self::Summary, Self::Progress, Self::Blockers]
    }

    pub fn as_path_segment(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Progress => "progress",
            Self::Blockers => "blockers",
        }
    }

    pub fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "summary" => Some(Self::Summary),
            "progress" => Some(Self::Progress),
            "blockers" => Some(Self::Blockers),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Summary => "route readiness summary",
            Self::Progress => "route readiness progress",
            Self::Blockers => "route readiness blockers",
        }
    }

    pub fn template(self) -> String {
        format!(
            "a2ex://readiness/routes/{{install_id}}/{{proposal_id}}/{{route_id}}/{}",
            self.as_path_segment()
        )
    }

    pub fn uri_for_route(self, install_id: &str, proposal_id: &str, route_id: &str) -> String {
        format!(
            "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/{}",
            self.as_path_segment()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoadBundleRequest {
    pub entry_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReloadBundleRequest {
    pub session_id: String,
    pub entry_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GenerateProposalPacketRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopSessionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClearStopRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoadBundleResponse {
    pub session_id: String,
    pub entry_url: String,
    pub session_uri_root: String,
    pub resource_uris: Vec<String>,
    pub prompt_names: Vec<String>,
    pub status: SessionInterpretationStatus,
    pub blocker_count: usize,
    pub ambiguity_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GenerateProposalPacketResponse {
    pub session_id: String,
    pub session_uri_root: String,
    pub proposal_uri: String,
    pub revision: u64,
    pub proposal_revision: u64,
    pub status: SessionInterpretationStatus,
    pub proposal_readiness: SessionProposalReadiness,
    pub blocker_count: usize,
    pub ambiguity_count: usize,
    pub capital_profile_completeness: SessionProposalCompleteness,
    pub cost_profile_completeness: SessionProposalCompleteness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadSessionResourceRequest {
    pub session_id: String,
    pub resource: SkillSessionResourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderPromptRequest {
    pub session_id: String,
    pub prompt_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderPromptResponse {
    pub name: String,
    pub session_id: String,
    pub referenced_resources: Vec<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSessionLifecycleSummary {
    pub classification: BundleLifecycleClassification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_bundle_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_bundle_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_compatible_daemon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_compatible_daemon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_documents: Vec<BundleDocumentLifecycleChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<BundleLifecycleDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSessionStatusResource {
    pub session_id: String,
    pub entry_url: String,
    pub session_uri_root: String,
    pub revision: u64,
    pub lifecycle: SkillSessionLifecycleSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSessionBundleResource {
    pub session_id: String,
    pub entry_url: String,
    pub session_uri_root: String,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<SkillBundle>,
    #[serde(default)]
    pub diagnostics: Vec<a2ex_skill_bundle::BundleDiagnostic>,
    pub lifecycle: SkillSessionLifecycleSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSessionLifecycleResource {
    pub session_id: String,
    pub entry_url: String,
    pub session_uri_root: String,
    pub revision: u64,
    pub lifecycle: BundleLifecycleChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionControlResponse {
    pub session_id: String,
    pub session_uri_root: String,
    pub revision: u64,
    pub stop_state: SessionStopState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at_ms: Option<u64>,
    pub operator_state_uri: String,
    pub failures_uri: String,
}

impl SessionControlResponse {
    fn from_snapshot(snapshot: &crate::session::SkillSessionSnapshot) -> Self {
        Self {
            session_id: snapshot.session_id.clone(),
            session_uri_root: snapshot.session_uri_root.clone(),
            revision: snapshot.revision,
            stop_state: snapshot.stop_state(),
            stopped_at_ms: snapshot.control.stopped_at_ms,
            operator_state_uri: SkillSessionResourceKind::OperatorState
                .uri_for_session(&snapshot.session_id),
            failures_uri: SkillSessionResourceKind::Failures.uri_for_session(&snapshot.session_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSessionOperatorStateResource {
    pub session_id: String,
    pub entry_url: String,
    pub session_uri_root: String,
    pub revision: u64,
    pub updated_at_ms: u64,
    pub stop_state: SessionStopState,
    pub stoppable: bool,
    pub clearable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at_ms: Option<u64>,
    pub blocker_count: usize,
    pub ambiguity_count: usize,
    pub required_owner_action_count: usize,
    pub proposal_readiness: SessionProposalReadiness,
    pub lifecycle_classification: BundleLifecycleClassification,
    pub lifecycle_diagnostic_count: usize,
    pub next_operator_step: SessionNextOperatorStep,
    pub last_command_outcome: SessionCommandOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSessionFailuresResource {
    pub session_id: String,
    pub entry_url: String,
    pub session_uri_root: String,
    pub revision: u64,
    pub updated_at_ms: u64,
    pub stop_state: SessionStopState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at_ms: Option<u64>,
    pub blocker_count: usize,
    pub ambiguity_count: usize,
    pub required_owner_action_count: usize,
    pub lifecycle_diagnostic_count: usize,
    pub current_failures: Vec<SessionFailureEvidence>,
    pub last_command_outcome: SessionCommandOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rejected_command: Option<SessionCommandRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStopState {
    Active,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionCommandDisposition {
    Succeeded,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCommandOutcome {
    pub command: String,
    pub observed_at_ms: u64,
    pub disposition: SessionCommandDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCommandRejection {
    pub command: String,
    pub observed_at_ms: u64,
    pub rejection_code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionNextOperatorStepKind {
    SupplyRequiredDocument,
    ResolveBlocker,
    ResolveAmbiguity,
    SatisfySetupRequirement,
    MakeOwnerDecision,
    InspectLifecycleDiagnostics,
    GenerateProposalPacket,
    ReviewProposalIncompleteness,
    ClearStop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionNextOperatorStep {
    pub kind: SessionNextOperatorStepKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionFailureKind {
    Blocker,
    Ambiguity,
    SetupRequirement,
    OwnerDecision,
    LifecycleDiagnostic,
    ProposalIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFailureEvidence {
    pub kind: SessionFailureKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
    pub owner_action_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<InterpretationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHandoff {
    pub install_id: String,
    pub workspace_id: String,
    pub attached_bundle_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionInterpretationStatus {
    InterpretedReady,
    NeedsSetup,
    NeedsOwnerDecision,
    Ambiguous,
    Blocked,
}

impl From<SkillBundleInterpretationStatus> for SessionInterpretationStatus {
    fn from(value: SkillBundleInterpretationStatus) -> Self {
        match value {
            SkillBundleInterpretationStatus::InterpretedReady => Self::InterpretedReady,
            SkillBundleInterpretationStatus::NeedsSetup => Self::NeedsSetup,
            SkillBundleInterpretationStatus::NeedsOwnerDecision => Self::NeedsOwnerDecision,
            SkillBundleInterpretationStatus::Ambiguous => Self::Ambiguous,
            SkillBundleInterpretationStatus::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionProposalReadiness {
    Ready,
    Incomplete,
    Blocked,
}

impl From<ProposalReadiness> for SessionProposalReadiness {
    fn from(value: ProposalReadiness) -> Self {
        match value {
            ProposalReadiness::Ready => Self::Ready,
            ProposalReadiness::Incomplete => Self::Incomplete,
            ProposalReadiness::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionProposalCompleteness {
    Complete,
    Unknown,
    RequiresOwnerInput,
    NotInBundleContract,
    Blocked,
}

impl From<ProposalQuantitativeCompleteness> for SessionProposalCompleteness {
    fn from(value: ProposalQuantitativeCompleteness) -> Self {
        match value {
            ProposalQuantitativeCompleteness::Complete => Self::Complete,
            ProposalQuantitativeCompleteness::Unknown => Self::Unknown,
            ProposalQuantitativeCompleteness::RequiresOwnerInput => Self::RequiresOwnerInput,
            ProposalQuantitativeCompleteness::NotInBundleContract => Self::NotInBundleContract,
            ProposalQuantitativeCompleteness::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillSessionResourceKind {
    Status,
    Bundle,
    Interpretation,
    Blockers,
    Ambiguities,
    Provenance,
    Lifecycle,
    Proposal,
    OperatorState,
    Failures,
}

impl SkillSessionResourceKind {
    pub fn all() -> [Self; 10] {
        [
            Self::Status,
            Self::Bundle,
            Self::Interpretation,
            Self::Blockers,
            Self::Ambiguities,
            Self::Provenance,
            Self::Lifecycle,
            Self::Proposal,
            Self::OperatorState,
            Self::Failures,
        ]
    }

    pub fn as_path_segment(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Bundle => "bundle",
            Self::Interpretation => "interpretation",
            Self::Blockers => "blockers",
            Self::Ambiguities => "ambiguities",
            Self::Provenance => "provenance",
            Self::Lifecycle => "lifecycle",
            Self::Proposal => "proposal",
            Self::OperatorState => "operator_state",
            Self::Failures => "failures",
        }
    }

    pub fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "status" => Some(Self::Status),
            "bundle" => Some(Self::Bundle),
            "interpretation" => Some(Self::Interpretation),
            "blockers" => Some(Self::Blockers),
            "ambiguities" => Some(Self::Ambiguities),
            "provenance" => Some(Self::Provenance),
            "lifecycle" => Some(Self::Lifecycle),
            "proposal" => Some(Self::Proposal),
            "operator_state" => Some(Self::OperatorState),
            "failures" => Some(Self::Failures),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Status => "session status",
            Self::Bundle => "session bundle",
            Self::Interpretation => "session interpretation",
            Self::Blockers => "session blockers",
            Self::Ambiguities => "session ambiguities",
            Self::Provenance => "session provenance",
            Self::Lifecycle => "session lifecycle",
            Self::Proposal => "session proposal",
            Self::OperatorState => "session operator state",
            Self::Failures => "session failures",
        }
    }

    pub fn uri_for_session(self, session_id: &str) -> String {
        format!(
            "{}/{}",
            session_uri_root(session_id),
            self.as_path_segment()
        )
    }

    pub fn template(self) -> String {
        format!(
            "a2ex://skills/sessions/{{session_id}}/{}",
            self.as_path_segment()
        )
    }
}

// McpContractError is defined in crate::error

pub fn skill_surface_contract() -> SkillSurfaceContract {
    let mut resources = SkillSessionResourceKind::all()
        .into_iter()
        .map(|resource| McpResourceDescriptor {
            uri_template: resource.template(),
            title: format!("skill session {}", resource.as_path_segment()),
            description: format!(
                "Read the {} view for a skill MCP session.",
                resource.as_path_segment()
            ),
        })
        .collect::<Vec<_>>();
    resources.extend(OnboardingResourceKind::all().into_iter().map(|resource| {
        McpResourceDescriptor {
            uri_template: resource.template(),
            title: format!("onboarding install {}", resource.as_path_segment()),
            description: format!(
                "Read the {} view for a canonical onboarding install.",
                resource.as_path_segment()
            ),
        }
    }));
    resources.extend(RuntimeControlResourceKind::all().into_iter().map(|resource| {
        McpResourceDescriptor {
            uri_template: resource.template(),
            title: format!("runtime control {}", resource.as_path_segment()),
            description: format!(
                "Read the {} view for canonical autonomous runtime control keyed by install identity.",
                resource.as_path_segment()
            ),
        }
    }));
    resources.extend(RouteReadinessResourceKind::all().into_iter().map(|resource| {
        McpResourceDescriptor {
            uri_template: resource.template(),
            title: format!("route readiness {}", resource.as_path_segment()),
            description: format!(
                "Read the {} view for canonical route readiness keyed by install/proposal/route identity.",
                resource.as_path_segment()
            ),
        }
    }));
    resources.extend(StrategySelectionResourceKind::all().into_iter().map(|resource| {
        McpResourceDescriptor {
            uri_template: resource.template(),
            title: format!("strategy selection {}", resource.as_path_segment()),
            description: format!(
                "Read the {} view for canonical strategy selection keyed by install/proposal/selection identity.",
                resource.as_path_segment()
            ),
        }
    }));
    resources.extend(StrategyRuntimeResourceKind::all().into_iter().map(|resource| {
        McpResourceDescriptor {
            uri_template: resource.template(),
            title: format!("strategy runtime {}", resource.as_path_segment()),
            description: format!(
                "Read the {} view for canonical approved-runtime inspection keyed by install/proposal/selection identity.",
                resource.as_path_segment()
            ),
        }
    }));

    let mut prompts = vec![
        McpPromptDescriptor {
            name: PROMPT_STATUS_SUMMARY.to_owned(),
            description: "Summarize the current skill session status from resource-backed state."
                .to_owned(),
            resource_templates: prompt_resource_kinds(PROMPT_STATUS_SUMMARY)
                .expect("known prompt")
                .into_iter()
                .map(SkillSessionResourceKind::template)
                .collect(),
        },
        McpPromptDescriptor {
            name: PROMPT_OWNER_GUIDANCE.to_owned(),
            description: "Summarize owner-facing next actions from the canonical skill session resources."
                .to_owned(),
            resource_templates: prompt_resource_kinds(PROMPT_OWNER_GUIDANCE)
                .expect("known prompt")
                .into_iter()
                .map(SkillSessionResourceKind::template)
                .collect(),
        },
        McpPromptDescriptor {
            name: PROMPT_PROPOSAL_PACKET.to_owned(),
            description: "Guide an agent through the owner-facing proposal packet using canonical session resources."
                .to_owned(),
            resource_templates: prompt_resource_kinds(PROMPT_PROPOSAL_PACKET)
                .expect("known prompt")
                .into_iter()
                .map(SkillSessionResourceKind::template)
                .collect(),
        },
        McpPromptDescriptor {
            name: PROMPT_OPERATOR_GUIDANCE.to_owned(),
            description: "Summarize operator control, stop state, and failure evidence from canonical session resources."
                .to_owned(),
            resource_templates: prompt_resource_kinds(PROMPT_OPERATOR_GUIDANCE)
                .expect("known prompt")
                .into_iter()
                .map(SkillSessionResourceKind::template)
                .collect(),
        },
    ];
    prompts.extend([
        McpPromptDescriptor {
            name: PROMPT_CURRENT_STEP_GUIDANCE.to_owned(),
            description: "Summarize the current guided onboarding step from canonical install-backed resources."
                .to_owned(),
            resource_templates: vec![
                OnboardingResourceKind::GuidedState.template(),
                OnboardingResourceKind::Diagnostics.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_FAILURE_SUMMARY.to_owned(),
            description: "Summarize onboarding blockers, drift, and the last action rejection from canonical install-backed resources."
                .to_owned(),
            resource_templates: vec![
                OnboardingResourceKind::Diagnostics.template(),
                OnboardingResourceKind::Checklist.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_ROUTE_READINESS_GUIDANCE.to_owned(),
            description: "Summarize canonical route readiness, where to inspect it, and the explicit owner action still required."
                .to_owned(),
            resource_templates: vec![
                RouteReadinessResourceKind::Summary.template(),
                RouteReadinessResourceKind::Progress.template(),
                RouteReadinessResourceKind::Blockers.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_ROUTE_BLOCKER_SUMMARY.to_owned(),
            description: "Summarize route-readiness blockers and incomplete evidence from canonical readiness resources."
                .to_owned(),
            resource_templates: vec![
                RouteReadinessResourceKind::Summary.template(),
                RouteReadinessResourceKind::Progress.template(),
                RouteReadinessResourceKind::Blockers.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_RUNTIME_CONTROL_GUIDANCE.to_owned(),
            description: "Summarize canonical autonomous runtime control, where to inspect it, and the explicit recovery action required."
                .to_owned(),
            resource_templates: vec![
                RuntimeControlResourceKind::Status.template(),
                RuntimeControlResourceKind::Failures.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE.to_owned(),
            description: "Point agents at the canonical operator report and linked runtime-control resources for one approved selection."
                .to_owned(),
            resource_templates: vec![
                StrategyRuntimeResourceKind::OperatorReport.template(),
                StrategyRuntimeResourceKind::Monitoring.template(),
                RuntimeControlResourceKind::Status.template(),
                RuntimeControlResourceKind::Failures.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE.to_owned(),
            description: "Point agents at the canonical recent-change report window and exception rollup for one approved selection."
                .to_owned(),
            resource_templates: vec![
                StrategyRuntimeResourceKind::ReportWindow.template(),
                StrategyRuntimeResourceKind::ExceptionRollup.template(),
                StrategyRuntimeResourceKind::OperatorReport.template(),
                RuntimeControlResourceKind::Status.template(),
                RuntimeControlResourceKind::Failures.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_STRATEGY_SELECTION_GUIDANCE.to_owned(),
            description: "Summarize canonical strategy-selection state, approved-runtime eligibility/monitoring, and which tools mutate override or approval state."
                .to_owned(),
            resource_templates: vec![
                StrategySelectionResourceKind::Summary.template(),
                StrategySelectionResourceKind::Overrides.template(),
                StrategySelectionResourceKind::Approval.template(),
                StrategySelectionResourceKind::Diff.template(),
                StrategySelectionResourceKind::ApprovalHistory.template(),
                StrategyRuntimeResourceKind::Eligibility.template(),
                StrategyRuntimeResourceKind::Monitoring.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_STRATEGY_SELECTION_DISCUSSION.to_owned(),
            description: "Summarize canonical recommendation basis, effective diff, and approval/reopen history for operator discussion."
                .to_owned(),
            resource_templates: vec![
                StrategySelectionResourceKind::Summary.template(),
                StrategySelectionResourceKind::Diff.template(),
                StrategySelectionResourceKind::ApprovalHistory.template(),
            ],
        },
        McpPromptDescriptor {
            name: PROMPT_STRATEGY_SELECTION_RECOVERY.to_owned(),
            description: "Summarize canonical diff, approval/reopen history, and live monitoring guidance for operator recovery."
                .to_owned(),
            resource_templates: vec![
                StrategySelectionResourceKind::Diff.template(),
                StrategySelectionResourceKind::ApprovalHistory.template(),
                StrategyRuntimeResourceKind::Eligibility.template(),
                StrategyRuntimeResourceKind::Monitoring.template(),
            ],
        },
    ]);

    SkillSurfaceContract {
        server_name: SERVER_NAME.to_owned(),
        tools: vec![
            McpToolDescriptor {
                name: TOOL_LOAD_BUNDLE.to_owned(),
                description: "Load a skill bundle URL into a session-oriented MCP surface."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_RELOAD_BUNDLE.to_owned(),
                description: "Reload an existing skill bundle session without changing its identity."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_GENERATE_PROPOSAL_PACKET.to_owned(),
                description: "Generate stable proposal metadata for an existing skill bundle session."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_STOP_SESSION.to_owned(),
                description: "Stop an intake session while preserving operator-state and failure resources."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_CLEAR_STOP.to_owned(),
                description: "Clear a stopped intake session without changing its stable identity."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_RUNTIME_STOP.to_owned(),
                description: "Stop the canonical autonomous runtime and keep blocked-state diagnostics inspectable."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_RUNTIME_PAUSE.to_owned(),
                description: "Pause the canonical autonomous runtime so no new autonomous actions begin."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_RUNTIME_CLEAR_STOP.to_owned(),
                description: "Clear paused or stopped autonomous runtime control without erasing failure evidence."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_BOOTSTRAP_INSTALL.to_owned(),
                description: "Bootstrap or reopen a canonical onboarding install and return install-scoped MCP resources."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_APPLY_ONBOARDING_ACTION.to_owned(),
                description: "Apply a supported guided onboarding action against the canonical install state."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_EVALUATE_ROUTE_READINESS.to_owned(),
                description: "Evaluate or refresh canonical route readiness for one install/proposal/route handoff."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_APPLY_ROUTE_READINESS_ACTION.to_owned(),
                description: "Apply a guided route-readiness owner action against canonical route-scoped state."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_STRATEGY_SELECTION_MATERIALIZE.to_owned(),
                description: "Materialize or reread the canonical strategy-selection record for one install/proposal handoff."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE.to_owned(),
                description: "Apply a typed owner override to the canonical strategy-selection record."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_STRATEGY_SELECTION_APPROVE.to_owned(),
                description: "Approve an exact canonical strategy-selection revision without replaying prior tool output."
                    .to_owned(),
            },
            McpToolDescriptor {
                name: TOOL_STRATEGY_SELECTION_REOPEN.to_owned(),
                description: "Reopen the canonical strategy-selection identity and return read-first operator resource URIs."
                    .to_owned(),
            },
        ],
        resources,
        prompts,
    }
}

pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities::builder()
        .enable_prompts()
        .enable_resources()
        .enable_tools()
        .build()
}

pub fn session_uri_root(session_id: &str) -> String {
    format!("a2ex://skills/sessions/{session_id}")
}

pub fn parse_session_resource_uri(
    uri: &str,
) -> Result<(String, SkillSessionResourceKind), McpContractError> {
    let parsed = Url::parse(uri).map_err(|source| McpContractError::InvalidSessionResourceUri {
        uri: uri.to_owned(),
        reason: source.to_string(),
    })?;

    if parsed.scheme() != "a2ex" {
        return Err(McpContractError::InvalidSessionResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected a2ex scheme, got {}", parsed.scheme()),
        });
    }

    if parsed.host_str() != Some("skills") {
        return Err(McpContractError::InvalidSessionResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected skills host, got {:?}", parsed.host_str()),
        });
    }

    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();

    if segments.len() != 3 || segments[0] != "sessions" {
        return Err(McpContractError::InvalidSessionResourceUri {
            uri: uri.to_owned(),
            reason: "expected path /sessions/{session_id}/{resource}".to_owned(),
        });
    }

    let resource = SkillSessionResourceKind::from_path_segment(segments[2]).ok_or_else(|| {
        McpContractError::InvalidSessionResourceUri {
            uri: uri.to_owned(),
            reason: format!("unknown resource segment {}", segments[2]),
        }
    })?;

    Ok((segments[1].to_owned(), resource))
}

pub fn parse_onboarding_resource_uri(
    uri: &str,
) -> Result<(String, OnboardingResourceKind), McpContractError> {
    let parsed =
        Url::parse(uri).map_err(|source| McpContractError::InvalidOnboardingResourceUri {
            uri: uri.to_owned(),
            reason: source.to_string(),
        })?;

    if parsed.scheme() != "a2ex" {
        return Err(McpContractError::InvalidOnboardingResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected a2ex scheme, got {}", parsed.scheme()),
        });
    }

    if parsed.host_str() != Some("onboarding") {
        return Err(McpContractError::InvalidOnboardingResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected onboarding host, got {:?}", parsed.host_str()),
        });
    }

    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();

    if segments.len() != 3 || segments[0] != "installs" {
        return Err(McpContractError::InvalidOnboardingResourceUri {
            uri: uri.to_owned(),
            reason: "expected path /installs/{install_id}/{resource}".to_owned(),
        });
    }

    let resource = OnboardingResourceKind::from_path_segment(segments[2]).ok_or_else(|| {
        McpContractError::InvalidOnboardingResourceUri {
            uri: uri.to_owned(),
            reason: format!("unknown resource segment {}", segments[2]),
        }
    })?;

    Ok((segments[1].to_owned(), resource))
}

pub fn parse_runtime_control_resource_uri(
    uri: &str,
) -> Result<(String, RuntimeControlResourceKind), McpContractError> {
    let parsed =
        Url::parse(uri).map_err(
            |source| McpContractError::InvalidRuntimeControlResourceUri {
                uri: uri.to_owned(),
                reason: source.to_string(),
            },
        )?;

    if parsed.scheme() != "a2ex" {
        return Err(McpContractError::InvalidRuntimeControlResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected a2ex scheme, got {}", parsed.scheme()),
        });
    }

    if parsed.host_str() != Some("runtime") {
        return Err(McpContractError::InvalidRuntimeControlResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected runtime host, got {:?}", parsed.host_str()),
        });
    }

    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();

    if segments.len() != 3 || segments[0] != "control" {
        return Err(McpContractError::InvalidRuntimeControlResourceUri {
            uri: uri.to_owned(),
            reason: "expected path /control/{install_id}/{resource}".to_owned(),
        });
    }

    let resource = RuntimeControlResourceKind::from_path_segment(segments[2]).ok_or_else(|| {
        McpContractError::InvalidRuntimeControlResourceUri {
            uri: uri.to_owned(),
            reason: format!("unknown resource segment {}", segments[2]),
        }
    })?;

    Ok((segments[1].to_owned(), resource))
}

pub fn parse_route_readiness_resource_uri(
    uri: &str,
) -> Result<(String, String, String, RouteReadinessResourceKind), McpContractError> {
    let parsed =
        Url::parse(uri).map_err(
            |source| McpContractError::InvalidRouteReadinessResourceUri {
                uri: uri.to_owned(),
                reason: source.to_string(),
            },
        )?;

    if parsed.scheme() != "a2ex" {
        return Err(McpContractError::InvalidRouteReadinessResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected a2ex scheme, got {}", parsed.scheme()),
        });
    }

    if parsed.host_str() != Some("readiness") {
        return Err(McpContractError::InvalidRouteReadinessResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected readiness host, got {:?}", parsed.host_str()),
        });
    }

    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();

    if segments.len() != 5 || segments[0] != "routes" {
        return Err(McpContractError::InvalidRouteReadinessResourceUri {
            uri: uri.to_owned(),
            reason: "expected path /routes/{install_id}/{proposal_id}/{route_id}/{resource}"
                .to_owned(),
        });
    }

    let resource = RouteReadinessResourceKind::from_path_segment(segments[4]).ok_or_else(|| {
        McpContractError::InvalidRouteReadinessResourceUri {
            uri: uri.to_owned(),
            reason: format!("unknown resource segment {}", segments[4]),
        }
    })?;

    Ok((
        segments[1].to_owned(),
        segments[2].to_owned(),
        segments[3].to_owned(),
        resource,
    ))
}

pub fn parse_strategy_selection_resource_uri(
    uri: &str,
) -> Result<(String, String, String, StrategySelectionResourceKind), McpContractError> {
    let parsed = Url::parse(uri).map_err(|source| {
        McpContractError::InvalidStrategySelectionResourceUri {
            uri: uri.to_owned(),
            reason: source.to_string(),
        }
    })?;

    if parsed.scheme() != "a2ex" {
        return Err(McpContractError::InvalidStrategySelectionResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected a2ex scheme, got {}", parsed.scheme()),
        });
    }

    if parsed.host_str() != Some("strategy-selection") {
        return Err(McpContractError::InvalidStrategySelectionResourceUri {
            uri: uri.to_owned(),
            reason: format!(
                "expected strategy-selection host, got {:?}",
                parsed.host_str()
            ),
        });
    }

    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();

    if segments.len() != 5 || segments[0] != "selections" {
        return Err(McpContractError::InvalidStrategySelectionResourceUri {
            uri: uri.to_owned(),
            reason:
                "expected path /selections/{install_id}/{proposal_id}/{selection_id}/{resource}"
                    .to_owned(),
        });
    }

    let resource =
        StrategySelectionResourceKind::from_path_segment(segments[4]).ok_or_else(|| {
            McpContractError::InvalidStrategySelectionResourceUri {
                uri: uri.to_owned(),
                reason: format!("unknown resource segment {}", segments[4]),
            }
        })?;

    Ok((
        segments[1].to_owned(),
        segments[2].to_owned(),
        segments[3].to_owned(),
        resource,
    ))
}

pub fn parse_strategy_runtime_resource_uri(
    uri: &str,
) -> Result<
    (
        String,
        String,
        String,
        StrategyRuntimeResourceKind,
        Option<String>,
    ),
    McpContractError,
> {
    let parsed =
        Url::parse(uri).map_err(
            |source| McpContractError::InvalidStrategyRuntimeResourceUri {
                uri: uri.to_owned(),
                reason: source.to_string(),
            },
        )?;

    if parsed.scheme() != "a2ex" {
        return Err(McpContractError::InvalidStrategyRuntimeResourceUri {
            uri: uri.to_owned(),
            reason: format!("expected a2ex scheme, got {}", parsed.scheme()),
        });
    }

    if parsed.host_str() != Some("strategy-runtime") {
        return Err(McpContractError::InvalidStrategyRuntimeResourceUri {
            uri: uri.to_owned(),
            reason: format!(
                "expected strategy-runtime host, got {:?}",
                parsed.host_str()
            ),
        });
    }

    let segments = parsed
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();

    if segments.len() < 5 || segments[0] != "selections" {
        return Err(McpContractError::InvalidStrategyRuntimeResourceUri {
            uri: uri.to_owned(),
            reason:
                "expected path /selections/{install_id}/{proposal_id}/{selection_id}/{resource}"
                    .to_owned(),
        });
    }

    let resource =
        StrategyRuntimeResourceKind::from_path_segment(segments[4]).ok_or_else(|| {
            McpContractError::InvalidStrategyRuntimeResourceUri {
                uri: uri.to_owned(),
                reason: format!("unknown resource segment {}", segments[4]),
            }
        })?;

    let cursor = match resource {
        StrategyRuntimeResourceKind::ReportWindow => {
            if segments.len() != 6 {
                return Err(McpContractError::InvalidStrategyRuntimeResourceUri {
                    uri: uri.to_owned(),
                    reason: "expected path /selections/{install_id}/{proposal_id}/{selection_id}/report-window/{cursor}"
                        .to_owned(),
                });
            }
            Some(segments[5].to_owned())
        }
        _ => {
            if segments.len() != 5 {
                return Err(McpContractError::InvalidStrategyRuntimeResourceUri {
                    uri: uri.to_owned(),
                    reason:
                        "expected path /selections/{install_id}/{proposal_id}/{selection_id}/{resource}"
                            .to_owned(),
                });
            }
            None
        }
    };

    Ok((
        segments[1].to_owned(),
        segments[2].to_owned(),
        segments[3].to_owned(),
        resource,
        cursor,
    ))
}

pub fn stable_session_id(entry_url: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(entry_url.as_bytes());
    hex::encode(hasher.finalize())[..16].to_owned()
}

fn prompt_resource_kinds(prompt_name: &str) -> Option<Vec<SkillSessionResourceKind>> {
    match prompt_name {
        PROMPT_STATUS_SUMMARY => Some(vec![
            SkillSessionResourceKind::Status,
            SkillSessionResourceKind::Interpretation,
        ]),
        PROMPT_OWNER_GUIDANCE => Some(vec![
            SkillSessionResourceKind::Status,
            SkillSessionResourceKind::Blockers,
            SkillSessionResourceKind::Ambiguities,
            SkillSessionResourceKind::Provenance,
            SkillSessionResourceKind::Interpretation,
        ]),
        PROMPT_PROPOSAL_PACKET => Some(vec![
            SkillSessionResourceKind::Proposal,
            SkillSessionResourceKind::Interpretation,
            SkillSessionResourceKind::Provenance,
        ]),
        PROMPT_OPERATOR_GUIDANCE => Some(vec![
            SkillSessionResourceKind::OperatorState,
            SkillSessionResourceKind::Failures,
        ]),
        _ => None,
    }
}

fn prompt_session_argument() -> PromptArgument {
    PromptArgument::new(PROMPT_ARGUMENT_SESSION_ID)
        .with_description("Stable MCP skill session id returned by skills.load_bundle")
        .with_required(true)
}

fn prompt_install_argument() -> PromptArgument {
    PromptArgument::new(PROMPT_ARGUMENT_INSTALL_ID)
        .with_description("Stable onboarding install id returned by onboarding.bootstrap_install")
        .with_required(true)
}

fn prompt_proposal_argument() -> PromptArgument {
    PromptArgument::new(PROMPT_ARGUMENT_PROPOSAL_ID)
        .with_description(
            "Stable proposal identity returned by skills.load_bundle and used for route readiness",
        )
        .with_required(true)
}

fn prompt_route_argument() -> PromptArgument {
    PromptArgument::new(PROMPT_ARGUMENT_ROUTE_ID)
        .with_description("Canonical route id for the concrete execution route being inspected")
        .with_required(true)
}

fn prompt_selection_argument() -> PromptArgument {
    PromptArgument::new(PROMPT_ARGUMENT_SELECTION_ID)
        .with_description(
            "Canonical strategy-selection id returned by strategy_selection.materialize",
        )
        .with_required(true)
}

fn prompt_session_id_argument(
    request: &GetPromptRequestParams,
) -> Result<String, McpContractError> {
    request
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.get(PROMPT_ARGUMENT_SESSION_ID))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_SESSION_ID,
        })
}

fn prompt_install_id_argument(
    request: &GetPromptRequestParams,
) -> Result<String, McpContractError> {
    request
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.get(PROMPT_ARGUMENT_INSTALL_ID))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_INSTALL_ID,
        })
}

fn prompt_route_readiness_arguments(
    request: &GetPromptRequestParams,
) -> Result<ReadRouteReadinessRequest, McpContractError> {
    let arguments =
        request
            .arguments
            .as_ref()
            .ok_or_else(|| McpContractError::MissingPromptArgument {
                prompt_name: request.name.clone(),
                argument_name: PROMPT_ARGUMENT_INSTALL_ID,
            })?;

    let install_id = arguments
        .get(PROMPT_ARGUMENT_INSTALL_ID)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_INSTALL_ID,
        })?;
    let proposal_id = arguments
        .get(PROMPT_ARGUMENT_PROPOSAL_ID)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_PROPOSAL_ID,
        })?;
    let route_id = arguments
        .get(PROMPT_ARGUMENT_ROUTE_ID)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_ROUTE_ID,
        })?;

    Ok(ReadRouteReadinessRequest {
        install_id,
        proposal_id,
        route_id,
    })
}

fn prompt_strategy_selection_arguments(
    request: &GetPromptRequestParams,
) -> Result<StrategySelectionReadRequest, McpContractError> {
    let arguments =
        request
            .arguments
            .as_ref()
            .ok_or_else(|| McpContractError::MissingPromptArgument {
                prompt_name: request.name.clone(),
                argument_name: PROMPT_ARGUMENT_INSTALL_ID,
            })?;

    let install_id = arguments
        .get(PROMPT_ARGUMENT_INSTALL_ID)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_INSTALL_ID,
        })?;
    let proposal_id = arguments
        .get(PROMPT_ARGUMENT_PROPOSAL_ID)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_PROPOSAL_ID,
        })?;
    let selection_id = arguments
        .get(PROMPT_ARGUMENT_SELECTION_ID)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpContractError::MissingPromptArgument {
            prompt_name: request.name.clone(),
            argument_name: PROMPT_ARGUMENT_SELECTION_ID,
        })?;

    Ok(StrategySelectionReadRequest {
        install_id,
        proposal_id,
        selection_id,
    })
}

fn resource_name_from_uri(uri: &str) -> String {
    uri.rsplit('/').next().unwrap_or("resource").to_owned()
}

fn onboarding_resource_payload(
    inspection: &GuidedOnboardingInspection,
    resource: OnboardingResourceKind,
) -> serde_json::Value {
    match resource {
        OnboardingResourceKind::GuidedState => {
            serde_json::to_value(OnboardingGuidedStateResource {
                install_id: inspection.install_id.clone(),
                workspace_id: inspection.workspace_id.clone(),
                attached_bundle_url: inspection.attached_bundle_url.to_string(),
                aggregate_status: inspection.aggregate_status,
                ordered_steps: inspection.ordered_steps.clone(),
                current_step_key: inspection.current_step_key.clone(),
                recommended_action: inspection.recommended_action.clone(),
                proposal_handoff: inspection.proposal_handoff.clone(),
            })
            .expect("guided onboarding state serializes")
        }
        OnboardingResourceKind::Checklist => serde_json::to_value(OnboardingChecklistResource {
            install_id: inspection.install_id.clone(),
            workspace_id: inspection.workspace_id.clone(),
            attached_bundle_url: inspection.attached_bundle_url.to_string(),
            aggregate_status: inspection.aggregate_status,
            checklist_items: inspection.checklist_items.clone(),
        })
        .expect("guided onboarding checklist serializes"),
        OnboardingResourceKind::Diagnostics => {
            serde_json::to_value(OnboardingDiagnosticsResource {
                install_id: inspection.install_id.clone(),
                workspace_id: inspection.workspace_id.clone(),
                attached_bundle_url: inspection.attached_bundle_url.to_string(),
                bootstrap: inspection.bootstrap.clone(),
                aggregate_status: inspection.aggregate_status,
                current_step_key: inspection.current_step_key.clone(),
                recommended_action: inspection.recommended_action.clone(),
                proposal_handoff: inspection.proposal_handoff.clone(),
                drift: inspection.drift.clone(),
                last_rejection: inspection.last_rejection.clone(),
            })
            .expect("guided onboarding diagnostics serializes")
        }
    }
}

fn runtime_control_resource_payload(
    install_id: &str,
    record: &PersistedRuntimeControl,
    resource: RuntimeControlResourceKind,
) -> serde_json::Value {
    let status_uri = RuntimeControlResourceKind::Status.uri_for_install(install_id);
    let failures_uri = RuntimeControlResourceKind::Failures.uri_for_install(install_id);

    match resource {
        RuntimeControlResourceKind::Status => serde_json::to_value(RuntimeControlStatusResource {
            install_id: install_id.to_owned(),
            scope_key: record.scope_key.clone(),
            control_mode: record.control_mode.clone(),
            autonomy_eligibility: runtime_control_autonomy_eligibility(&record.control_mode)
                .to_owned(),
            transition_reason: record.transition_reason.clone(),
            transition_source: record.transition_source.clone(),
            transitioned_at: record.transitioned_at.clone(),
            last_cleared_at: record.last_cleared_at.clone(),
            last_cleared_reason: record.last_cleared_reason.clone(),
            last_cleared_source: record.last_cleared_source.clone(),
            updated_at: record.updated_at.clone(),
            status_uri,
            failures_uri,
        })
        .expect("runtime control status serializes"),
        RuntimeControlResourceKind::Failures => {
            serde_json::to_value(RuntimeControlFailuresResource {
                install_id: install_id.to_owned(),
                scope_key: record.scope_key.clone(),
                control_mode: record.control_mode.clone(),
                autonomy_eligibility: runtime_control_autonomy_eligibility(&record.control_mode)
                    .to_owned(),
                transition_reason: record.transition_reason.clone(),
                transition_source: record.transition_source.clone(),
                transitioned_at: record.transitioned_at.clone(),
                last_cleared_at: record.last_cleared_at.clone(),
                last_cleared_reason: record.last_cleared_reason.clone(),
                last_cleared_source: record.last_cleared_source.clone(),
                last_rejection: runtime_control_last_rejection(record),
                updated_at: record.updated_at.clone(),
                status_uri,
                failures_uri,
            })
            .expect("runtime control failures serializes")
        }
    }
}

fn route_readiness_resource_payload(
    record: &RouteReadinessRecord,
    resource: RouteReadinessResourceKind,
) -> serde_json::Value {
    let summary_uri = RouteReadinessResourceKind::Summary.uri_for_route(
        &record.identity.install_id,
        &record.identity.proposal_id,
        &record.identity.route_id,
    );
    let progress_uri = RouteReadinessResourceKind::Progress.uri_for_route(
        &record.identity.install_id,
        &record.identity.proposal_id,
        &record.identity.route_id,
    );
    let blockers_uri = RouteReadinessResourceKind::Blockers.uri_for_route(
        &record.identity.install_id,
        &record.identity.proposal_id,
        &record.identity.route_id,
    );

    match resource {
        RouteReadinessResourceKind::Summary => {
            serde_json::to_value(RouteReadinessSummaryResource {
                record: record.clone(),
                summary_uri,
                progress_uri,
                blockers_uri,
            })
            .expect("route readiness summary serializes")
        }
        RouteReadinessResourceKind::Progress => {
            serde_json::to_value(RouteReadinessProgressResource {
                identity: record.identity.clone(),
                status: record.status,
                ordered_steps: record.ordered_steps.clone(),
                current_step_key: record.current_step_key.clone(),
                recommended_action: record.recommended_action.clone(),
                stale: record.stale.clone(),
                last_rejection: record.last_rejection.clone(),
                evaluation: record.evaluation.clone(),
                evaluated_at: record.evaluated_at.clone(),
                summary_uri,
                progress_uri,
                blockers_uri,
            })
            .expect("route readiness progress serializes")
        }
        RouteReadinessResourceKind::Blockers => {
            serde_json::to_value(RouteReadinessBlockersResource {
                identity: record.identity.clone(),
                status: record.status,
                blockers: record.blockers.clone(),
                recommended_owner_action: record.recommended_owner_action.clone(),
                stale: record.stale.clone(),
                last_rejection: record.last_rejection.clone(),
                evaluation: record.evaluation.clone(),
                evaluated_at: record.evaluated_at.clone(),
                summary_uri,
                progress_uri,
                blockers_uri,
            })
            .expect("route readiness blockers serializes")
        }
    }
}

fn strategy_selection_resource_payload(
    inspection: &StrategySelectionInspection,
    resource: StrategySelectionResourceKind,
) -> serde_json::Value {
    let summary_uri = StrategySelectionResourceKind::Summary.uri_for_selection(
        &inspection.summary.install_id,
        &inspection.summary.proposal_id,
        &inspection.summary.selection_id,
    );
    let overrides_uri = StrategySelectionResourceKind::Overrides.uri_for_selection(
        &inspection.summary.install_id,
        &inspection.summary.proposal_id,
        &inspection.summary.selection_id,
    );
    let approval_uri = StrategySelectionResourceKind::Approval.uri_for_selection(
        &inspection.summary.install_id,
        &inspection.summary.proposal_id,
        &inspection.summary.selection_id,
    );
    let diff_uri = StrategySelectionResourceKind::Diff.uri_for_selection(
        &inspection.summary.install_id,
        &inspection.summary.proposal_id,
        &inspection.summary.selection_id,
    );
    let approval_history_uri = StrategySelectionResourceKind::ApprovalHistory.uri_for_selection(
        &inspection.summary.install_id,
        &inspection.summary.proposal_id,
        &inspection.summary.selection_id,
    );

    match resource {
        StrategySelectionResourceKind::Summary => {
            serde_json::to_value(StrategySelectionSummaryResource {
                summary: inspection.summary.clone(),
                summary_uri,
                overrides_uri,
                approval_uri,
                diff_uri,
                approval_history_uri,
            })
            .expect("strategy selection summary serializes")
        }
        StrategySelectionResourceKind::Overrides => {
            serde_json::to_value(StrategySelectionOverridesResource {
                install_id: inspection.summary.install_id.clone(),
                proposal_id: inspection.summary.proposal_id.clone(),
                selection_id: inspection.summary.selection_id.clone(),
                selection_revision: inspection.summary.selection_revision,
                status: inspection.summary.status.as_str().to_owned(),
                overrides: inspection.overrides.clone(),
                summary_uri,
                overrides_uri,
                approval_uri,
                diff_uri,
                approval_history_uri,
            })
            .expect("strategy selection overrides serializes")
        }
        StrategySelectionResourceKind::Approval => {
            serde_json::to_value(StrategySelectionApprovalResource {
                install_id: inspection.summary.install_id.clone(),
                proposal_id: inspection.summary.proposal_id.clone(),
                selection_id: inspection.summary.selection_id.clone(),
                selection_revision: inspection.summary.selection_revision,
                status: inspection.summary.status.as_str().to_owned(),
                approved_revision: inspection.summary.approval.approved_revision,
                approved_by: inspection.summary.approval.approved_by.clone(),
                note: inspection.summary.approval.note.clone(),
                approved_at: inspection.summary.approval.approved_at.clone(),
                summary_uri,
                overrides_uri,
                approval_uri,
                diff_uri,
                approval_history_uri,
            })
            .expect("strategy selection approval serializes")
        }
        StrategySelectionResourceKind::Diff => {
            serde_json::to_value(StrategySelectionDiffResource {
                install_id: inspection.summary.install_id.clone(),
                proposal_id: inspection.summary.proposal_id.clone(),
                selection_id: inspection.summary.selection_id.clone(),
                selection_revision: inspection.summary.selection_revision,
                status: inspection.summary.status.as_str().to_owned(),
                baseline_kind: inspection.effective_diff.baseline_kind.clone(),
                changed_override_keys: inspection.effective_diff.changed_override_keys.clone(),
                readiness_sensitive_changes: inspection
                    .effective_diff
                    .readiness_sensitive_changes
                    .clone(),
                advisory_changes: inspection.effective_diff.advisory_changes.clone(),
                readiness_stale: inspection.effective_diff.readiness_stale,
                approval_stale: inspection.effective_diff.approval_stale,
                approval_stale_reason: inspection.effective_diff.approval_stale_reason.clone(),
                summary_uri,
                overrides_uri,
                approval_uri,
                diff_uri,
                approval_history_uri,
            })
            .expect("strategy selection diff serializes")
        }
        StrategySelectionResourceKind::ApprovalHistory => {
            serde_json::to_value(StrategySelectionApprovalHistoryResource {
                install_id: inspection.summary.install_id.clone(),
                proposal_id: inspection.summary.proposal_id.clone(),
                selection_id: inspection.summary.selection_id.clone(),
                selection_revision: inspection.summary.selection_revision,
                status: inspection.summary.status.as_str().to_owned(),
                events: inspection.approval_history.clone(),
                summary_uri,
                overrides_uri,
                approval_uri,
                diff_uri,
                approval_history_uri,
            })
            .expect("strategy selection approval history serializes")
        }
    }
}

fn strategy_runtime_eligibility_resource_payload(
    handoff: &StrategyRuntimeHandoffRecord,
) -> StrategyRuntimeEligibilityResource {
    StrategyRuntimeEligibilityResource {
        handoff: handoff.clone(),
        eligibility_uri: StrategyRuntimeResourceKind::Eligibility.uri_for_selection(
            &handoff.install_id,
            &handoff.proposal_id,
            &handoff.selection_id,
        ),
        monitoring_uri: StrategyRuntimeResourceKind::Monitoring.uri_for_selection(
            &handoff.install_id,
            &handoff.proposal_id,
            &handoff.selection_id,
        ),
    }
}

fn strategy_runtime_monitoring_resource_payload(
    monitoring: &StrategyRuntimeMonitoringSummary,
) -> StrategyRuntimeMonitoringResource {
    StrategyRuntimeMonitoringResource {
        monitoring: monitoring.clone(),
        eligibility_uri: StrategyRuntimeResourceKind::Eligibility.uri_for_selection(
            &monitoring.handoff.install_id,
            &monitoring.handoff.proposal_id,
            &monitoring.handoff.selection_id,
        ),
        monitoring_uri: StrategyRuntimeResourceKind::Monitoring.uri_for_selection(
            &monitoring.handoff.install_id,
            &monitoring.handoff.proposal_id,
            &monitoring.handoff.selection_id,
        ),
    }
}

fn strategy_operator_report_resource_payload(
    report: &StrategyOperatorReport,
    request: &StrategySelectionReadRequest,
) -> StrategyOperatorReportResource {
    StrategyOperatorReportResource {
        report: report.clone(),
        operator_report_uri: StrategyRuntimeResourceKind::OperatorReport.uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        ),
        eligibility_uri: StrategyRuntimeResourceKind::Eligibility.uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        ),
        monitoring_uri: StrategyRuntimeResourceKind::Monitoring.uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        ),
        runtime_control_status_uri: RuntimeControlResourceKind::Status
            .uri_for_install(&request.install_id),
        runtime_control_failures_uri: RuntimeControlResourceKind::Failures
            .uri_for_install(&request.install_id),
    }
}

fn strategy_report_window_resource_payload(
    report_window: &StrategyReportWindow,
    request: &StrategySelectionReadRequest,
    cursor: &str,
) -> StrategyReportWindowResource {
    StrategyReportWindowResource {
        report_window: report_window.clone(),
        report_window_uri: StrategyRuntimeResourceKind::report_window_uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
            cursor,
        ),
        exception_rollup_uri: StrategyRuntimeResourceKind::ExceptionRollup.uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        ),
        operator_report_uri: StrategyRuntimeResourceKind::OperatorReport.uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        ),
    }
}

fn strategy_exception_rollup_resource_payload(
    exception_rollup: &StrategyExceptionRollup,
    request: &StrategySelectionReadRequest,
) -> StrategyExceptionRollupResource {
    StrategyExceptionRollupResource {
        report_kind: exception_rollup.report_kind.clone(),
        identity: exception_rollup.identity.clone(),
        owner_action_needed_now: exception_rollup.owner_action_needed_now,
        urgency: exception_rollup.urgency,
        recommended_operator_action: exception_rollup.recommended_operator_action.clone(),
        active_hold: exception_rollup.active_hold.clone(),
        last_runtime_failure: exception_rollup.last_runtime_failure.clone(),
        last_runtime_rejection: exception_rollup.last_runtime_rejection.clone(),
        exception_rollup_uri: StrategyRuntimeResourceKind::ExceptionRollup.uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        ),
        bootstrap_report_window_uri: StrategyRuntimeResourceKind::report_window_uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
            "bootstrap",
        ),
        operator_report_uri: StrategyRuntimeResourceKind::OperatorReport.uri_for_selection(
            &request.install_id,
            &request.proposal_id,
            &request.selection_id,
        ),
    }
}

fn render_onboarding_prompt(
    prompt_name: &str,
    inspection: &GuidedOnboardingInspection,
) -> Result<RenderPromptResponse, RmcpError> {
    let guided_uri = OnboardingResourceKind::GuidedState.uri_for_install(&inspection.install_id);
    let checklist_uri = OnboardingResourceKind::Checklist.uri_for_install(&inspection.install_id);
    let diagnostics_uri =
        OnboardingResourceKind::Diagnostics.uri_for_install(&inspection.install_id);
    let (referenced_resources, content) = match prompt_name {
        PROMPT_CURRENT_STEP_GUIDANCE => {
            let step = inspection
                .current_step_key
                .clone()
                .unwrap_or_else(|| "ready".to_owned());
            let action = inspection
                .recommended_action
                .as_ref()
                .map(|action| {
                    serde_json::to_string(&action.kind)
                        .unwrap_or_else(|_| "\"unknown\"".to_owned())
                        .trim_matches('"')
                        .to_owned()
                })
                .unwrap_or_else(|| "none".to_owned());
            let handoff = inspection
                .proposal_handoff
                .as_ref()
                .map(|handoff| {
                    format!(
                        " Onboarding is ready for proposal intake. Call {} with entry_url={} next, then call {} and inspect {}. Prompt {} is the owner-facing proposal guide.",
                        handoff.tool_name,
                        handoff.entry_url,
                        handoff.next_tool_name,
                        handoff.proposal_resource_template,
                        handoff.prompt_name,
                    )
                })
                .unwrap_or_default();
            (
                vec![guided_uri.clone(), diagnostics_uri.clone()],
                format!(
                    "Install {} current guided step: {}. Aggregate status: {}. Attached bundle URL: {}. Recommended action: {}. Read guided state {} and diagnostics {} before taking action.{}",
                    inspection.install_id,
                    step,
                    inspection.aggregate_status.as_str(),
                    inspection.attached_bundle_url,
                    action,
                    guided_uri,
                    diagnostics_uri,
                    handoff,
                ),
            )
        }
        PROMPT_FAILURE_SUMMARY => {
            let drift = inspection
                .drift
                .as_ref()
                .map(|drift| {
                    serde_json::to_string(&drift.classification)
                        .unwrap_or_else(|_| "\"unknown\"".to_owned())
                        .trim_matches('"')
                        .to_owned()
                })
                .unwrap_or_else(|| "none".to_owned());
            let rejection = inspection
                .last_rejection
                .as_ref()
                .map(|rejection| format!(" Last rejection: {}.", rejection.code))
                .unwrap_or_default();
            let checklist_keys = inspection
                .checklist_items
                .iter()
                .map(|item| item.checklist_key.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (
                vec![diagnostics_uri.clone(), checklist_uri.clone()],
                format!(
                    "Install {} failure summary. Aggregate status: {}. Current step: {}. Drift: {}. Attached bundle URL: {}. used_remote_control_plane={}. Checklist keys: [{}]. Review diagnostics {} and checklist {}.{}",
                    inspection.install_id,
                    inspection.aggregate_status.as_str(),
                    inspection
                        .current_step_key
                        .clone()
                        .unwrap_or_else(|| "ready".to_owned()),
                    drift,
                    inspection.attached_bundle_url,
                    inspection.bootstrap.used_remote_control_plane,
                    checklist_keys,
                    diagnostics_uri,
                    checklist_uri,
                    rejection,
                ),
            )
        }
        other => {
            return Err(RmcpError::invalid_params(
                format!("unknown onboarding prompt {other}"),
                Some(serde_json::json!({ "prompt_name": other })),
            ));
        }
    };

    Ok(RenderPromptResponse {
        name: prompt_name.to_owned(),
        session_id: inspection.install_id.clone(),
        referenced_resources,
        content,
    })
}

fn render_route_readiness_prompt(
    prompt_name: &str,
    record: &RouteReadinessRecord,
    request: &ReadRouteReadinessRequest,
) -> Result<RenderPromptResponse, RmcpError> {
    let summary_uri = RouteReadinessResourceKind::Summary.uri_for_route(
        &request.install_id,
        &request.proposal_id,
        &request.route_id,
    );
    let progress_uri = RouteReadinessResourceKind::Progress.uri_for_route(
        &request.install_id,
        &request.proposal_id,
        &request.route_id,
    );
    let blockers_uri = RouteReadinessResourceKind::Blockers.uri_for_route(
        &request.install_id,
        &request.proposal_id,
        &request.route_id,
    );
    let action = record
        .recommended_owner_action
        .as_ref()
        .map(|action| format!("{} — {}", action.kind, action.summary))
        .unwrap_or_else(|| "none".to_owned());
    let blocker_codes = if record.blockers.is_empty() {
        "none".to_owned()
    } else {
        record
            .blockers
            .iter()
            .map(|blocker| blocker.code.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let last_rejection = record
        .last_rejection
        .as_ref()
        .map(|rejection| format!("{}: {}", rejection.code, rejection.message))
        .unwrap_or_else(|| "none".to_owned());
    let stale = record
        .stale
        .as_ref()
        .map(|stale| stale.status.as_str())
        .unwrap_or("fresh");

    let completeness = serde_json::to_string(&record.capital.completeness)
        .unwrap_or_else(|_| "\"unknown\"".to_owned())
        .trim_matches('"')
        .to_owned();

    let runtime_status_uri =
        RuntimeControlResourceKind::Status.uri_for_install(&request.install_id);
    let runtime_failures_uri =
        RuntimeControlResourceKind::Failures.uri_for_install(&request.install_id);
    let ready_runtime_handoff = if record.status.as_str() == "ready"
        && record.current_step_key.is_none()
    {
        format!(
            " Route is fully ready for runtime handoff. Inspect runtime status {} and failures {}. Use {} for explicit runtime blocking control, and reread {} with install_id={} for runtime recovery guidance before autonomous execution.",
            runtime_status_uri,
            runtime_failures_uri,
            TOOL_RUNTIME_STOP,
            PROMPT_RUNTIME_CONTROL_GUIDANCE,
            request.install_id,
        )
    } else {
        String::new()
    };

    let content = match prompt_name {
        PROMPT_ROUTE_READINESS_GUIDANCE => format!(
            "Route readiness for install {}, proposal {}, route {} is {}. Inspect summary {}, progress {}, and blockers {} from canonical state. Use {} for owner-progress mutation; keep it separate from {} reevaluation. Capital completeness: {}. Required approval tuples: {}. Next owner action: {}. Stale readiness status: {}.{} Do not invent approval authority or bypass the local signing boundary.",
            request.install_id,
            request.proposal_id,
            request.route_id,
            record.status.as_str(),
            summary_uri,
            progress_uri,
            blockers_uri,
            TOOL_APPLY_ROUTE_READINESS_ACTION,
            TOOL_EVALUATE_ROUTE_READINESS,
            completeness,
            record.approvals.len(),
            action,
            stale,
            ready_runtime_handoff,
        ),
        PROMPT_ROUTE_BLOCKER_SUMMARY => format!(
            "Route readiness blocker summary for install {}, proposal {}, route {}. Status: {}. Blocker codes: {}. Capital completeness: {}. Inspect blockers {}, progress {}, and cross-check summary {}. Required owner action: {}. Durable last_rejection diagnostics: {}. Stale readiness review status: {}. Keep blocked or incomplete evidence explicit; do not claim hidden approval authority.",
            request.install_id,
            request.proposal_id,
            request.route_id,
            record.status.as_str(),
            blocker_codes,
            completeness,
            blockers_uri,
            progress_uri,
            summary_uri,
            action,
            last_rejection,
            stale,
        ),
        other => {
            return Err(RmcpError::invalid_params(
                format!("unknown route readiness prompt {other}"),
                Some(serde_json::json!({ "prompt_name": other })),
            ));
        }
    };

    Ok(RenderPromptResponse {
        name: prompt_name.to_owned(),
        session_id: request.proposal_id.clone(),
        referenced_resources: vec![summary_uri, progress_uri, blockers_uri],
        content,
    })
}

fn render_strategy_operator_report_prompt(
    report: &StrategyOperatorReport,
    request: &StrategySelectionReadRequest,
) -> RenderPromptResponse {
    let operator_report_uri = StrategyRuntimeResourceKind::OperatorReport.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let monitoring_uri = StrategyRuntimeResourceKind::Monitoring.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let eligibility_uri = StrategyRuntimeResourceKind::Eligibility.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let runtime_control_status_uri =
        RuntimeControlResourceKind::Status.uri_for_install(&request.install_id);
    let runtime_control_failures_uri =
        RuntimeControlResourceKind::Failures.uri_for_install(&request.install_id);
    let content = format!(
        "Operator report guidance for selection {}. Reread canonical local truth at operator report {}, then use runtime control status {} and runtime control failures {} to confirm whether the latest operator action is caused by an active hold, an execution failure, or a runtime-control rejection. Cross-check approved-runtime context at eligibility {} and monitoring {} when you need deeper provenance. Current recommended operator action: {}. Current control mode: {}. Do not rely on prior mutation receipts or session memory.",
        request.selection_id,
        operator_report_uri,
        runtime_control_status_uri,
        runtime_control_failures_uri,
        eligibility_uri,
        monitoring_uri,
        report.recommended_operator_action,
        report.control_mode,
    );
    RenderPromptResponse {
        name: PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE.to_owned(),
        session_id: request.proposal_id.clone(),
        referenced_resources: vec![
            operator_report_uri,
            runtime_control_status_uri,
            runtime_control_failures_uri,
            eligibility_uri,
            monitoring_uri,
        ],
        content,
    }
}

fn render_strategy_report_window_prompt(
    report_window: &StrategyReportWindow,
    exception_rollup: &StrategyExceptionRollup,
    request: &StrategySelectionReadRequest,
) -> RenderPromptResponse {
    let report_window_uri = StrategyRuntimeResourceKind::report_window_uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
        "bootstrap",
    );
    let exception_rollup_uri = StrategyRuntimeResourceKind::ExceptionRollup.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let operator_report_uri = StrategyRuntimeResourceKind::OperatorReport.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let runtime_control_status_uri =
        RuntimeControlResourceKind::Status.uri_for_install(&request.install_id);
    let runtime_control_failures_uri =
        RuntimeControlResourceKind::Failures.uri_for_install(&request.install_id);
    let runtime_control_handoff = format!(
        " When a runtime-control hold or rejection is present, hand off into {} and reread runtime control status {} plus runtime control failures {} before deciding whether to stop, pause, or clear_stop. Current runtime-control evidence should be compared against reason codes like {}.",
        PROMPT_RUNTIME_CONTROL_GUIDANCE,
        runtime_control_status_uri,
        runtime_control_failures_uri,
        exception_rollup
            .last_runtime_rejection
            .as_ref()
            .map(|rejection| rejection.code.as_str())
            .or_else(|| {
                exception_rollup
                    .active_hold
                    .as_ref()
                    .map(|hold| hold.reason_code.as_str())
            })
            .unwrap_or("none"),
    );
    let content = format!(
        "Report-window guidance for selection {}. Reread canonical local truth at report window {} using an explicit cursor, then reread exception rollup {} before deciding whether the owner needs to intervene now. The current bootstrap window ends at {} and the current recommended operator action is {}. Cross-check operator report {} when you need the full current runtime snapshot.{} Do not rely on prior mutation receipts or session memory.",
        request.selection_id,
        report_window_uri,
        exception_rollup_uri,
        report_window.window_end_cursor,
        exception_rollup.recommended_operator_action,
        operator_report_uri,
        runtime_control_handoff,
    );
    RenderPromptResponse {
        name: PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE.to_owned(),
        session_id: request.proposal_id.clone(),
        referenced_resources: vec![
            report_window_uri,
            exception_rollup_uri,
            operator_report_uri,
            runtime_control_status_uri,
            runtime_control_failures_uri,
        ],
        content,
    }
}

fn render_runtime_control_prompt(
    install_id: &str,
    record: &PersistedRuntimeControl,
    current_selection: Option<&StrategySelectionReadRequest>,
) -> RenderPromptResponse {
    let status_uri = RuntimeControlResourceKind::Status.uri_for_install(install_id);
    let failures_uri = RuntimeControlResourceKind::Failures.uri_for_install(install_id);
    let rejection = runtime_control_last_rejection(record)
        .map(|rejection| {
            format!(
                " Last rejection: {} — {} (attempted_operation={}, rejected_at={}).",
                rejection.code,
                rejection.message,
                rejection.attempted_operation,
                rejection.rejected_at,
            )
        })
        .unwrap_or_default();

    let autonomy_eligibility = runtime_control_autonomy_eligibility(&record.control_mode);
    let mut referenced_resources = vec![status_uri.clone(), failures_uri.clone()];
    let supervision_handoff = current_selection
        .map(|selection| {
            let operator_report_uri = StrategyRuntimeResourceKind::OperatorReport.uri_for_selection(
                &selection.install_id,
                &selection.proposal_id,
                &selection.selection_id,
            );
            let report_window_uri =
                StrategyRuntimeResourceKind::report_window_uri_for_selection(
                    &selection.install_id,
                    &selection.proposal_id,
                    &selection.selection_id,
                    "bootstrap",
                );
            let exception_rollup_uri =
                StrategyRuntimeResourceKind::ExceptionRollup.uri_for_selection(
                    &selection.install_id,
                    &selection.proposal_id,
                    &selection.selection_id,
                );
            referenced_resources.extend([
                operator_report_uri.clone(),
                report_window_uri.clone(),
                exception_rollup_uri.clone(),
            ]);
            format!(
                " For the current approved-runtime supervision loop, reread operator report {}, report window {}, and exception rollup {} after every control transition so stop/clear-stop recovery stays tied to canonical local truth rather than mutation receipts.",
                operator_report_uri,
                report_window_uri,
                exception_rollup_uri,
            )
        })
        .unwrap_or_else(|| {
            " No approved-runtime supervision selection is currently available for install-scoped report handoff; rely on canonical runtime control resources until onboarding rehydrates one.".to_owned()
        });

    RenderPromptResponse {
        name: PROMPT_RUNTIME_CONTROL_GUIDANCE.to_owned(),
        session_id: install_id.to_owned(),
        referenced_resources,
        content: format!(
            "Runtime control for install {} is {}. Autonomy eligibility: {}. Inspect status {} and failures {} from canonical local state before acting. Use {} to stop autonomy, {} to pause new autonomous actions, and {} for explicit recovery before autonomy becomes eligible again. Transition reason: {}. Transition source: {}.{}{}",
            install_id,
            record.control_mode,
            autonomy_eligibility,
            status_uri,
            failures_uri,
            TOOL_RUNTIME_STOP,
            TOOL_RUNTIME_PAUSE,
            TOOL_RUNTIME_CLEAR_STOP,
            record.transition_reason,
            record.transition_source,
            rejection,
            supervision_handoff,
        ),
    }
}

fn render_strategy_selection_prompt(
    prompt_name: &str,
    inspection: &StrategySelectionInspection,
    runtime_eligibility: Option<&StrategyRuntimeHandoffRecord>,
    runtime_monitoring: Option<&StrategyRuntimeMonitoringSummary>,
    request: &StrategySelectionReadRequest,
) -> RenderPromptResponse {
    let summary_uri = StrategySelectionResourceKind::Summary.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let overrides_uri = StrategySelectionResourceKind::Overrides.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let approval_uri = StrategySelectionResourceKind::Approval.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let diff_uri = StrategySelectionResourceKind::Diff.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let approval_history_uri = StrategySelectionResourceKind::ApprovalHistory.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let eligibility_uri = StrategyRuntimeResourceKind::Eligibility.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );
    let monitoring_uri = StrategyRuntimeResourceKind::Monitoring.uri_for_selection(
        &request.install_id,
        &request.proposal_id,
        &request.selection_id,
    );

    let mut referenced_resources = vec![
        summary_uri.clone(),
        overrides_uri.clone(),
        approval_uri.clone(),
        diff_uri.clone(),
        approval_history_uri.clone(),
    ];
    if runtime_eligibility.is_some() || runtime_monitoring.is_some() {
        referenced_resources.push(eligibility_uri.clone());
        referenced_resources.push(monitoring_uri.clone());
    }

    let hold_reason = runtime_eligibility
        .and_then(|eligibility| eligibility.hold_reason)
        .map(|reason| reason.as_str().to_owned())
        .or_else(|| inspection.summary.approval_stale_reason.clone())
        .unwrap_or_else(|| "none".to_owned());
    let monitoring_phase = runtime_monitoring
        .map(|monitoring| format!("{:?}", monitoring.current_phase))
        .unwrap_or_else(|| "unavailable".to_owned());
    let last_guidance = runtime_monitoring
        .and_then(|monitoring| monitoring.last_operator_guidance.as_ref())
        .map(|guidance| format!("{} — {}", guidance.recommended_action, guidance.summary))
        .unwrap_or_else(|| "none".to_owned());
    let discussion_basis = format!(
        "{}@{}",
        inspection.discussion.recommendation_basis.source_kind,
        inspection.discussion.recommendation_basis.proposal_revision
    );

    let content = match prompt_name {
        PROMPT_STRATEGY_SELECTION_GUIDANCE => format!(
            "Strategy selection {} for install {} proposal {} is {} at revision {}. Inspect summary {}, overrides {}, approval {}, diff {}, and approval history {} from canonical local state after every reconnect. Approved-runtime rereads live at eligibility {} and monitoring {}. Hold reason: {}. Monitoring phase: {}. Use {} to record typed owner changes, {} to approve the exact current revision, {} to reopen the same selection identity when operator review must restart, and {} to reread or materialize the canonical record when proposal state is ready. Do not rely on prior mutation receipts or session memory.",
            request.selection_id,
            request.install_id,
            request.proposal_id,
            inspection.summary.status.as_str(),
            inspection.summary.selection_revision,
            summary_uri,
            overrides_uri,
            approval_uri,
            diff_uri,
            approval_history_uri,
            eligibility_uri,
            monitoring_uri,
            hold_reason,
            monitoring_phase,
            TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE,
            TOOL_STRATEGY_SELECTION_APPROVE,
            TOOL_STRATEGY_SELECTION_REOPEN,
            TOOL_STRATEGY_SELECTION_MATERIALIZE,
        ),
        PROMPT_STRATEGY_SELECTION_DISCUSSION => format!(
            "Operator discussion for selection {}. Recommendation basis: {}. Inspect summary {}, diff {}, and approval history {} to explain why the current canonical record looks the way it does. Changed override keys: {:?}. Approval stale reason: {}. If the operator changes a typed assumption, use {} and then reread {}. Do not rely on prior mutation receipts or session memory.",
            request.selection_id,
            discussion_basis,
            summary_uri,
            diff_uri,
            approval_history_uri,
            inspection.effective_diff.changed_override_keys,
            inspection
                .effective_diff
                .approval_stale_reason
                .clone()
                .unwrap_or_else(|| "none".to_owned()),
            TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE,
            diff_uri,
        ),
        PROMPT_STRATEGY_SELECTION_RECOVERY => format!(
            "Operator recovery for selection {} after reconnect. Reread diff {} and approval history {} first, then inspect eligibility {} and monitoring {} from canonical local state. Current hold reason: {}. Monitoring phase: {}. Last operator guidance: {}. Use {} to reopen the same selection identity when explicit operator review is required, {} to approve the exact current revision once the diff is acceptable, and {} to refresh runtime recovery status. Do not rely on prior mutation receipts or session memory.",
            request.selection_id,
            diff_uri,
            approval_history_uri,
            eligibility_uri,
            monitoring_uri,
            hold_reason,
            monitoring_phase,
            last_guidance,
            TOOL_STRATEGY_SELECTION_REOPEN,
            TOOL_STRATEGY_SELECTION_APPROVE,
            monitoring_uri,
        ),
        _ => unreachable!("validated upstream"),
    };

    RenderPromptResponse {
        name: prompt_name.to_owned(),
        session_id: request.proposal_id.clone(),
        referenced_resources,
        content,
    }
}

fn render_status_summary(snapshot: &crate::session::SkillSessionSnapshot) -> String {
    format!(
        "Session {} is {:?}. Blockers: {}. Ambiguities: {}. Resource root: {}.",
        snapshot.session_id,
        snapshot.status(),
        snapshot.blocker_count(),
        snapshot.ambiguity_count(),
        snapshot.session_uri_root
    )
}

fn render_owner_guidance(snapshot: &crate::session::SkillSessionSnapshot) -> String {
    let mut lines = vec![
        format!("Session {} owner guidance", snapshot.session_id),
        format!("Status: {:?}", snapshot.status()),
    ];

    if !snapshot.interpretation.blockers.is_empty() {
        lines.push("Blockers:".to_owned());
        lines.extend(snapshot.interpretation.blockers.iter().map(|blocker| {
            format!(
                "- {} ({})",
                blocker.summary,
                blocker
                    .diagnostic_code
                    .as_ref()
                    .map(|code| serde_json::to_string(code).unwrap_or_else(|_| "null".to_owned()))
                    .unwrap_or_else(|| "null".to_owned())
                    .trim_matches('"')
            )
        }));
    }

    if !snapshot.interpretation.setup_requirements.is_empty() {
        lines.push("Setup requirements:".to_owned());
        lines.extend(
            snapshot
                .interpretation
                .setup_requirements
                .iter()
                .map(|requirement| format!("- {}", requirement.requirement_key)),
        );
    }

    if !snapshot.interpretation.owner_decisions.is_empty() {
        lines.push("Owner decisions:".to_owned());
        lines.extend(
            snapshot
                .interpretation
                .owner_decisions
                .iter()
                .map(|decision| format!("- {}", decision.decision_text)),
        );
    }

    if !snapshot.interpretation.ambiguities.is_empty() {
        lines.push("Ambiguities:".to_owned());
        lines.extend(
            snapshot
                .interpretation
                .ambiguities
                .iter()
                .map(|ambiguity| format!("- {}", ambiguity.summary)),
        );
    }

    if !snapshot.interpretation.automation_boundaries.is_empty() {
        lines.push("Automation boundaries:".to_owned());
        lines.extend(
            snapshot
                .interpretation
                .automation_boundaries
                .iter()
                .map(|boundary| format!("- {}", boundary.summary)),
        );
    }

    lines.push(format!(
        "Inspect resources under {}",
        snapshot.session_uri_root
    ));
    lines.join("\n")
}

fn render_proposal_prompt(
    snapshot: &crate::session::SkillSessionSnapshot,
) -> Result<String, McpContractError> {
    let proposal = generate_proposal_packet(&snapshot.outcome, &snapshot.interpretation)?;
    Ok(format!(
        "Use the proposal packet for session {} as the canonical owner-facing truth. Review proposal resource {} first, then cross-check interpretation {} and provenance {}. Proposal readiness: {:?}. Capital completeness: {:?}. Cost completeness: {:?}. Keep blockers, ambiguities, and provenance explicit; do not invent quantitative values that the bundle contract does not provide.",
        snapshot.session_id,
        SkillSessionResourceKind::Proposal.uri_for_session(&snapshot.session_id),
        SkillSessionResourceKind::Interpretation.uri_for_session(&snapshot.session_id),
        SkillSessionResourceKind::Provenance.uri_for_session(&snapshot.session_id),
        proposal.proposal_readiness,
        proposal.capital_profile.completeness,
        proposal.cost_profile.completeness,
    ))
}

fn render_operator_guidance(
    snapshot: &crate::session::SkillSessionSnapshot,
) -> Result<String, McpContractError> {
    let operator_state = snapshot.operator_state_resource()?;
    let failures = snapshot.failures_resource()?;
    let mut lines = vec![
        format!("Session {} operator guidance", snapshot.session_id),
        format!(
            "Inspect operator_state {} and failures {} first.",
            SkillSessionResourceKind::OperatorState.uri_for_session(&snapshot.session_id),
            SkillSessionResourceKind::Failures.uri_for_session(&snapshot.session_id)
        ),
        format!("Stop state: {:?}", operator_state.stop_state),
        format!(
            "Required owner actions: {}",
            operator_state.required_owner_action_count
        ),
        format!(
            "Proposal readiness: {:?}",
            operator_state.proposal_readiness
        ),
        format!(
            "Next operator step: {:?} — {}",
            operator_state.next_operator_step.kind, operator_state.next_operator_step.summary
        ),
    ];

    if let Some(first_failure) = failures.current_failures.first() {
        lines.push(format!(
            "Primary failure: {} ({})",
            first_failure.summary,
            first_failure
                .diagnostic_code
                .clone()
                .unwrap_or_else(|| "unknown".to_owned())
        ));
    }

    if let Some(rejected) = failures.last_rejected_command {
        lines.push(format!(
            "Last rejected command: {} ({})",
            rejected.command, rejected.rejection_code
        ));
    }

    Ok(lines.join("\n"))
}

fn runtime_control_last_rejection(
    record: &PersistedRuntimeControl,
) -> Option<RuntimeControlRejection> {
    Some(RuntimeControlRejection {
        code: record.last_rejection_code.clone()?,
        message: record.last_rejection_message.clone()?,
        attempted_operation: record.last_rejection_operation.clone()?,
        rejected_at: record.last_rejection_at.clone()?,
    })
}

fn runtime_control_autonomy_eligibility(control_mode: &str) -> &'static str {
    match control_mode {
        "active" => "eligible",
        _ => "blocked",
    }
}

/// Resolve a human-readable chain name to its chain ID.
pub(crate) fn resolve_chain_id(chain_name: &str) -> u64 {
    match chain_name.to_lowercase().as_str() {
        "ethereum" | "mainnet" | "eth" => 1,
        "polygon" | "matic" => 137,
        "arbitrum" | "arb" => 42161,
        "optimism" | "op" => 10,
        "base" => 8453,
        _ => 1, // default to mainnet
    }
}

/// Resolve a Hyperliquid asset symbol to its numeric index.
/// This is a simplified mapping; production would use a registry.
pub(crate) fn resolve_hyperliquid_asset_index(symbol: &str) -> u32 {
    match symbol.to_uppercase().as_str() {
        "BTC" => 0,
        "ETH" => 1,
        "SOL" => 2,
        "AVAX" => 3,
        "ARB" => 4,
        "OP" => 5,
        "MATIC" => 6,
        "DOGE" => 7,
        "LINK" => 8,
        _ => {
            // Try parsing as a numeric index directly
            symbol.parse::<u32>().unwrap_or(0)
        }
    }
}

fn default_runtime_control_record() -> PersistedRuntimeControl {
    PersistedRuntimeControl {
        scope_key: AUTONOMOUS_RUNTIME_CONTROL_SCOPE.to_owned(),
        control_mode: "active".to_owned(),
        transition_reason: "initial_state".to_owned(),
        transition_source: "daemon".to_owned(),
        transitioned_at: "2026-03-11T00:00:00Z".to_owned(),
        last_cleared_at: None,
        last_cleared_reason: None,
        last_cleared_source: None,
        last_rejection_code: None,
        last_rejection_message: None,
        last_rejection_operation: None,
        last_rejection_at: None,
        updated_at: "2026-03-11T00:00:00Z".to_owned(),
    }
}

fn normalize_interpretation(
    mut interpretation: SkillBundleInterpretation,
) -> SkillBundleInterpretation {
    interpretation.blockers = dedupe_blockers(interpretation.blockers);
    interpretation.provenance = dedupe_provenance(interpretation.provenance);
    interpretation
}

fn apply_ready_onboarding_handoff(
    mut interpretation: SkillBundleInterpretation,
    handoff: Option<&SessionHandoff>,
) -> SkillBundleInterpretation {
    if handoff.is_none() {
        return interpretation;
    }

    if matches!(
        interpretation.status,
        SkillBundleInterpretationStatus::Blocked | SkillBundleInterpretationStatus::Ambiguous
    ) {
        return interpretation;
    }

    interpretation.status = SkillBundleInterpretationStatus::InterpretedReady;
    interpretation.setup_requirements.clear();
    interpretation
}

fn dedupe_blockers(blockers: Vec<InterpretationBlocker>) -> Vec<InterpretationBlocker> {
    let mut unique: Vec<InterpretationBlocker> = Vec::new();
    for blocker in blockers {
        if let Some(existing) = unique
            .iter_mut()
            .find(|existing| existing.blocker_key == blocker.blocker_key)
        {
            for evidence in blocker.evidence {
                if !existing.evidence.contains(&evidence) {
                    existing.evidence.push(evidence);
                }
            }
            if existing.diagnostic_code.is_none() {
                existing.diagnostic_code = blocker.diagnostic_code;
            }
            if existing.diagnostic_phase.is_none() {
                existing.diagnostic_phase = blocker.diagnostic_phase;
            }
            if existing.diagnostic_severity.is_none() {
                existing.diagnostic_severity = blocker.diagnostic_severity;
            }
            continue;
        }

        unique.push(blocker);
    }
    unique
}

fn dedupe_provenance(provenance: Vec<InterpretationEvidence>) -> Vec<InterpretationEvidence> {
    let mut unique = Vec::new();
    for evidence in provenance {
        if !unique.contains(&evidence) {
            unique.push(evidence);
        }
    }
    unique
}

fn parse_entry_url(entry_url: &str) -> Result<Url, McpContractError> {
    Url::parse(entry_url).map_err(|source| McpContractError::InvalidEntryUrl {
        entry_url: entry_url.to_owned(),
        source,
    })
}

// mcp_contract_to_rmcp_error is defined in crate::error

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_resource_uri_round_trips() {
        let uri = SkillSessionResourceKind::Proposal.uri_for_session("abc123");
        let (session_id, resource) = parse_session_resource_uri(&uri).expect("uri parses");

        assert_eq!(session_id, "abc123");
        assert_eq!(resource, SkillSessionResourceKind::Proposal);
    }

    #[test]
    fn server_capabilities_enable_tools_resources_and_prompts() {
        let capabilities = server_capabilities();

        assert!(capabilities.tools.is_some());
        assert!(capabilities.resources.is_some());
        assert!(capabilities.prompts.is_some());
    }

    #[test]
    fn route_readiness_resource_uri_round_trips() {
        let uri = RouteReadinessResourceKind::Summary.uri_for_route(
            "install-1",
            "proposal-1",
            "planned_execution:across:kalshi:hyperliquid:polygon",
        );
        let (install_id, proposal_id, route_id, resource) =
            parse_route_readiness_resource_uri(&uri).expect("uri parses");

        assert_eq!(install_id, "install-1");
        assert_eq!(proposal_id, "proposal-1");
        assert_eq!(
            route_id,
            "planned_execution:across:kalshi:hyperliquid:polygon"
        );
        assert_eq!(resource, RouteReadinessResourceKind::Summary);
    }

    #[test]
    fn strategy_selection_resource_uri_round_trips() {
        let uri = StrategySelectionResourceKind::Approval.uri_for_selection(
            "install-1",
            "proposal-1",
            "selection-1",
        );
        let (install_id, proposal_id, selection_id, resource) =
            parse_strategy_selection_resource_uri(&uri).expect("uri parses");

        assert_eq!(install_id, "install-1");
        assert_eq!(proposal_id, "proposal-1");
        assert_eq!(selection_id, "selection-1");
        assert_eq!(resource, StrategySelectionResourceKind::Approval);
    }

    #[test]
    fn proposal_prompt_references_canonical_resources() {
        let resources = prompt_resource_kinds(PROMPT_PROPOSAL_PACKET).expect("known prompt");
        assert_eq!(
            resources,
            vec![
                SkillSessionResourceKind::Proposal,
                SkillSessionResourceKind::Interpretation,
                SkillSessionResourceKind::Provenance,
            ]
        );
    }
}

// ---------------------------------------------------------------------------
// Environment-based server construction
// ---------------------------------------------------------------------------

/// Build an [`A2exSkillMcpServer`] from environment variables.
///
/// When the four required env vars are present and non-empty the server is
/// constructed with real HTTP venue adapters.  When any required var is
/// missing the server falls back to [`A2exSkillMcpServer::default()`].
///
/// # Required env vars
///
/// | Variable | Purpose |
/// |---|---|
/// | `A2EX_WAIAAS_BASE_URL` | WAIaaS signer base URL |
/// | `A2EX_HOT_SESSION_TOKEN` | Hot-wallet session token |
/// | `A2EX_HOT_WALLET_ID` | Hot-wallet identifier |
/// | `A2EX_WAIAAS_NETWORK` | Network name (`mainnet`, `testnet`, …) |
///
/// # Optional env vars
///
/// | Variable | Default when absent |
/// |---|---|
/// | `A2EX_ACROSS_INTEGRATOR_ID` | `None` |
/// | `A2EX_ACROSS_API_KEY` | `None` |
/// | `A2EX_HYPERLIQUID_BASE_URL` | `https://api.hyperliquid.xyz` |
/// | `A2EX_POLYMARKET_CLOB_BASE_URL` | [`VenueAdapters::DEFAULT_POLYMARKET_CLOB_BASE_URL`] |
///
/// All diagnostic output goes to **stderr** (stdout is reserved for MCP
/// stdio transport).  Secret values are never logged — only `<set>` markers.
pub fn build_server_from_env() -> A2exSkillMcpServer {
    use a2ex_across_adapter::transport::AcrossHttpTransport;
    use a2ex_hyperliquid_adapter::HyperliquidHttpTransport;
    use a2ex_waiaas_signer::WaiaasSignerBridge;

    const REQUIRED: [&str; 4] = [
        "A2EX_WAIAAS_BASE_URL",
        "A2EX_HOT_SESSION_TOKEN",
        "A2EX_HOT_WALLET_ID",
        "A2EX_WAIAAS_NETWORK",
    ];

    let vals: Vec<Option<String>> = REQUIRED
        .iter()
        .map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .collect();

    let missing: Vec<&str> = REQUIRED
        .iter()
        .zip(vals.iter())
        .filter_map(|(k, v)| if v.is_none() { Some(*k) } else { None })
        .collect();

    if !missing.is_empty() {
        eprintln!(
            "venue adapters: not configured (missing env vars: {})",
            missing.join(", ")
        );
        return A2exSkillMcpServer::default();
    }

    // All required vars present — unwrap safe after missing-check above.
    let waiaas_base_url = vals[0].clone().unwrap();
    let hot_session_token = vals[1].clone().unwrap();
    let hot_wallet_id = vals[2].clone().unwrap();
    let waiaas_network = vals[3].clone().unwrap();

    let is_mainnet = matches!(
        waiaas_network.as_str(),
        "mainnet"
            | "ethereum-mainnet"
            | "arbitrum-mainnet"
            | "polygon-mainnet"
            | "base-mainnet"
    );

    // Log presence (never values) to stderr.
    eprintln!("venue adapters: configuring");
    eprintln!("  A2EX_WAIAAS_BASE_URL=<set>");
    eprintln!("  A2EX_HOT_SESSION_TOKEN=<set>");
    eprintln!("  A2EX_HOT_WALLET_ID=<set>");
    eprintln!("  A2EX_WAIAAS_NETWORK={waiaas_network}");

    // -- Signer ---------------------------------------------------------------
    let signer = Arc::new(WaiaasSignerBridge::new(
        &waiaas_base_url,
        &hot_session_token,
        &hot_wallet_id,
        &waiaas_network,
    ));

    // -- Across ---------------------------------------------------------------
    let across_integrator_id = std::env::var("A2EX_ACROSS_INTEGRATOR_ID")
        .ok()
        .filter(|v| !v.is_empty());
    let across_api_key = std::env::var("A2EX_ACROSS_API_KEY")
        .ok()
        .filter(|v| !v.is_empty());
    let across_transport = Arc::new(AcrossHttpTransport::new(
        "https://app.across.to/api",
        across_integrator_id,
        across_api_key,
    ));
    let across_adapter = AcrossAdapter::with_transport(across_transport, 0);

    // -- Hyperliquid ----------------------------------------------------------
    let hl_base_url = std::env::var("A2EX_HYPERLIQUID_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://api.hyperliquid.xyz".to_owned());
    let hl_transport = HyperliquidHttpTransport::new(&hl_base_url, signer.clone(), is_mainnet);
    let hyperliquid_adapter = HyperliquidAdapter::with_transport(Arc::new(hl_transport), 0);

    // -- Polymarket (prediction market) ---------------------------------------
    // Credentials start as None; derived later via venue.derive_api_key.
    let prediction_market_adapter = PredictionMarketAdapter::default();

    let polymarket_clob_base_url = std::env::var("A2EX_POLYMARKET_CLOB_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty());

    // -- Assemble -------------------------------------------------------------
    let mut venue_adapters = VenueAdapters::new(
        across_adapter,
        hyperliquid_adapter,
        prediction_market_adapter,
        signer,
    );

    if let Some(clob_url) = polymarket_clob_base_url {
        venue_adapters = venue_adapters.with_polymarket_clob_base_url(clob_url);
    }

    eprintln!("venue adapters: configured");
    A2exSkillMcpServer::with_venue_adapters(venue_adapters)
}
