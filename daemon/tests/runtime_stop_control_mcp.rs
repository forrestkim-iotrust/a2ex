mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_evm_adapter::NoopEvmAdapter;
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_mcp::{A2exSkillMcpServer, PROMPT_ARGUMENT_INSTALL_ID, SERVER_NAME};
use a2ex_onboarding::{ClaimDisposition, InstallBootstrapRequest, bootstrap_install};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
};
use a2ex_state::StateRepository;
use async_trait::async_trait;
use reqwest::Url;
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams, ResourceContents,
    },
    transport::TokioChildProcess,
};
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};
use support::{
    hyperliquid_harness::FakeHyperliquidTransport,
    skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle},
};
use tempfile::tempdir;

const TOOL_RUNTIME_STOP: &str = "runtime.stop";
const TOOL_RUNTIME_PAUSE: &str = "runtime.pause";
const TOOL_RUNTIME_CLEAR_STOP: &str = "runtime.clear_stop";
const RESOURCE_RUNTIME_STATUS_TEMPLATE: &str = "a2ex://runtime/control/{install_id}/status";
const RESOURCE_RUNTIME_FAILURES_TEMPLATE: &str = "a2ex://runtime/control/{install_id}/failures";
const PROMPT_RUNTIME_CONTROL_GUIDANCE: &str = "runtime.control_guidance";
const RUNTIME_STATUS_URI_PREFIX: &str = "a2ex://runtime/control";
const RUNTIME_CONTROL_TABLE: &str = "runtime_control";
const RUNTIME_CONTROL_SCOPE: &str = "autonomous_runtime";
const RUNTIME_CONTROL_COLUMNS: &[&str] = &[
    "scope_key",
    "control_mode",
    "transition_reason",
    "transition_source",
    "transitioned_at",
    "last_cleared_at",
    "last_cleared_reason",
    "last_cleared_source",
    "last_rejection_code",
    "last_rejection_message",
    "last_rejection_operation",
    "last_rejection_at",
    "updated_at",
];

const READY_PATH_ENTRY_SKILL_MD: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.12
compatible_daemon: ">=0.1.0"
name: Prediction Spread Arb
summary: Capture spread dislocations between prediction venues.
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
    revision: 2026.03.10
---
# Overview

Track spread divergences after setup is complete.

# Owner Decisions

- Approve max spread budget.
"#;

const READY_PATH_OWNER_SETUP_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

#[derive(Default, Clone)]
struct RecordingSigner;

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, _request: &ExecutionRequest) {}
}

#[derive(Default, Clone)]
struct ApprovingBridge;

#[async_trait]
impl SignerBridge for ApprovingBridge {
    async fn request_approval(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalResult, a2ex_signer_bridge::SignerBridgeError> {
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(request),
        })
    }
}

#[derive(Debug, Clone)]
struct RuntimeControlRow {
    control_mode: String,
    transition_reason: String,
    transition_source: String,
    transitioned_at: String,
    last_cleared_at: Option<String>,
    last_cleared_reason: Option<String>,
    last_cleared_source: Option<String>,
    last_rejection_code: Option<String>,
    last_rejection_message: Option<String>,
    last_rejection_operation: Option<String>,
    last_rejection_at: Option<String>,
    updated_at: String,
}

#[tokio::test]
async fn runtime_control_mcp_surface_must_advertise_dedicated_tools_resources_and_guidance_prompt()
{
    let mut gaps = Vec::new();
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before runtime control handlers are implemented");

    if !capabilities
        .tools
        .iter()
        .any(|tool| tool.name == TOOL_RUNTIME_STOP)
    {
        gaps.push(
            "MCP capabilities must advertise runtime.stop for explicit runtime halt control"
                .to_owned(),
        );
    }
    if !capabilities
        .tools
        .iter()
        .any(|tool| tool.name == TOOL_RUNTIME_PAUSE)
    {
        gaps.push(
            "MCP capabilities must advertise runtime.pause for explicit no-new-actions control"
                .to_owned(),
        );
    }
    if !capabilities
        .tools
        .iter()
        .any(|tool| tool.name == TOOL_RUNTIME_CLEAR_STOP)
    {
        gaps.push(
            "MCP capabilities must advertise runtime.clear_stop for explicit blocked-state recovery"
                .to_owned(),
        );
    }
    if !capabilities
        .resources
        .iter()
        .any(|resource| resource.uri_template == RESOURCE_RUNTIME_STATUS_TEMPLATE)
    {
        gaps.push(
            "MCP capabilities must advertise a runtime status resource template backed by canonical state.db truth"
                .to_owned(),
        );
    }
    if !capabilities
        .resources
        .iter()
        .any(|resource| resource.uri_template == RESOURCE_RUNTIME_FAILURES_TEMPLATE)
    {
        gaps.push(
            "MCP capabilities must advertise a runtime failures resource template for durable blocked-action diagnostics"
                .to_owned(),
        );
    }
    if !capabilities
        .prompts
        .iter()
        .any(|prompt| prompt.name == PROMPT_RUNTIME_CONTROL_GUIDANCE)
    {
        gaps.push(
            "MCP capabilities must advertise runtime.control_guidance so agents can reread recovery guidance without replaying mutation text"
                .to_owned(),
        );
    }

    assert!(
        gaps.is_empty(),
        "S03 MCP runtime control surface is missing advertised capabilities: {}",
        gaps.join("; ")
    );
}

#[tokio::test]
async fn runtime_control_mcp_resources_and_prompts_must_reread_canonical_state_across_reconnect() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let install_bootstrap = bootstrap_install(InstallBootstrapRequest {
        install_url: Url::parse(&entry_url).expect("entry url parses"),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: None,
        expected_install_id: None,
    })
    .await
    .expect("canonical install bootstrap should create the workspace runtime state");
    assert!(matches!(
        install_bootstrap.claim_disposition,
        ClaimDisposition::Claimed
    ));

    let service = runtime_daemon_service(workspace_root.path()).await;
    register_strategy(&service).await;
    service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![a2ex_strategy_runtime::RuntimeWatcherState {
                watcher_key: "w-1".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-12T00:00:10Z".to_owned(),
            }],
            "2026-03-12T00:00:10Z",
        )
        .await
        .expect("runtime flow evaluates before MCP control surfaces inspect it");
    let repository = StateRepository::open(runtime_config(workspace_root.path()).state_db_path())
        .await
        .expect("runtime repo opens");
    let runtime_snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("runtime snapshot loads")
        .expect("registered strategy snapshot exists");
    assert_eq!(runtime_snapshot.strategy.strategy_id, "strategy-lp-1");

    let state_db_path = runtime_config(workspace_root.path())
        .state_db_path()
        .to_path_buf();
    let state = Connection::open(&state_db_path).expect("workspace state db opens");
    let existing_columns = table_columns(&state, RUNTIME_CONTROL_TABLE);
    let mut gaps = Vec::new();
    if existing_columns.is_empty() {
        gaps.push(
            "workspace state.db is missing the canonical runtime_control table required for MCP runtime inspection"
                .to_owned(),
        );
    } else {
        require_columns(&existing_columns, RUNTIME_CONTROL_COLUMNS, &mut gaps);
    }
    ensure_contract_table_for_test_progression(&state);
    upsert_runtime_control(
        &state,
        RuntimeControlRow {
            control_mode: "stopped".to_owned(),
            transition_reason: "operator_stop".to_owned(),
            transition_source: "runtime_stop_control_mcp".to_owned(),
            transitioned_at: "2026-03-12T00:02:00Z".to_owned(),
            last_cleared_at: None,
            last_cleared_reason: None,
            last_cleared_source: None,
            last_rejection_code: Some("runtime_stopped".to_owned()),
            last_rejection_message: Some(
                "runtime is stopped; clear_stop before executing new strategy actions (manual_stop aligned)"
                    .to_owned(),
            ),
            last_rejection_operation: Some("strategy_rebalance".to_owned()),
            last_rejection_at: Some("2026-03-12T00:02:05Z".to_owned()),
            updated_at: "2026-03-12T00:02:05Z".to_owned(),
        },
    );

    let first_client = spawn_live_client().await;
    let install_id = bootstrap_install_live(
        &first_client,
        &entry_url,
        workspace_root.path(),
        None,
        Some(install_bootstrap.install_id.clone()),
    )
    .await["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();
    let workspace_id = install_bootstrap.workspace_id.clone();
    let status_uri = format!("{RUNTIME_STATUS_URI_PREFIX}/{install_id}/status");
    let failures_uri = format!("{RUNTIME_STATUS_URI_PREFIX}/{install_id}/failures");

    match read_resource_json_result(&first_client, status_uri.clone()).await {
        Ok(status) => {
            if status["control_mode"] != "stopped" {
                gaps.push(format!(
                    "runtime status resource must expose control_mode=stopped, found {}",
                    status["control_mode"]
                ));
            }
            if status["autonomy_eligibility"] != "blocked" {
                gaps.push(
                    "runtime status resource must expose autonomy_eligibility=blocked while stopped"
                        .to_owned(),
                );
            }
            if status["transition_reason"] != "operator_stop" {
                gaps.push(
                    "runtime status resource must expose transition_reason from canonical state.db"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime status resource {} should be readable from canonical install state, got {error}",
            status_uri
        )),
    }

    match read_resource_json_result(&first_client, failures_uri.clone()).await {
        Ok(failures) => {
            if failures["control_mode"] != "stopped"
                || failures["autonomy_eligibility"] != "blocked"
                || failures["transition_reason"] != "operator_stop"
                || failures["transition_source"] != "runtime_stop_control_mcp"
                || failures["transitioned_at"] != "2026-03-12T00:02:00Z"
            {
                gaps.push(
                    "runtime failures resource must mirror canonical stopped transition facts, eligibility, and timestamps"
                        .to_owned(),
                );
            }
            if failures["last_rejection"]["code"] != "runtime_stopped" {
                gaps.push(
                    "runtime failures resource must expose last_rejection.code=runtime_stopped"
                        .to_owned(),
                );
            }
            if failures["last_rejection"]["attempted_operation"] != "strategy_rebalance" {
                gaps.push(
                    "runtime failures resource must expose last_rejection.attempted_operation=strategy_rebalance"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime failures resource {} should be readable from canonical install state, got {error}",
            failures_uri
        )),
    }

    match render_prompt_result(&first_client, install_id.clone()).await {
        Ok(prompt_text) => {
            if !prompt_text.contains(TOOL_RUNTIME_CLEAR_STOP) {
                gaps.push(format!(
                    "runtime control guidance prompt must reference {} for explicit recovery",
                    TOOL_RUNTIME_CLEAR_STOP
                ));
            }
            if !prompt_text.contains(&status_uri) || !prompt_text.contains(&failures_uri) {
                gaps.push(
                    "runtime control guidance prompt must point agents at canonical status and failures resources"
                        .to_owned(),
                );
            }
            if !prompt_text.contains("Autonomy eligibility: blocked") {
                gaps.push(
                    "runtime control guidance prompt must describe blocked autonomy eligibility while stopped"
                        .to_owned(),
                );
            }
            if !prompt_text.contains("runtime_stopped") {
                gaps.push(
                    "runtime control guidance prompt must include the durable runtime_stopped rejection code"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime control guidance prompt should render from canonical install state, got {error}"
        )),
    }

    first_client
        .cancel()
        .await
        .expect("first live stdio server should shut down cleanly");

    let reconnected_client = spawn_live_client().await;
    match read_resource_json_result(&reconnected_client, status_uri.clone()).await {
        Ok(status) => gaps.push(format!(
            "runtime status resource must reject reconnect reads before onboarding.bootstrap_install repopulates the install locator, got {status:?}"
        )),
        Err(error) => {
            if !(error.contains("install") || error.contains("locator")) {
                gaps.push(format!(
                    "runtime status pre-reopen rejection must mention install or locator context, got {error}"
                ));
            }
        }
    }
    match read_resource_json_result(&reconnected_client, failures_uri.clone()).await {
        Ok(failures) => gaps.push(format!(
            "runtime failures resource must reject reconnect reads before onboarding.bootstrap_install repopulates the install locator, got {failures:?}"
        )),
        Err(error) => {
            if !(error.contains("install") || error.contains("locator")) {
                gaps.push(format!(
                    "runtime failures pre-reopen rejection must mention install or locator context, got {error}"
                ));
            }
        }
    }
    let reopened = bootstrap_install_live(
        &reconnected_client,
        &entry_url,
        workspace_root.path(),
        Some(workspace_id),
        Some(install_id.clone()),
    )
    .await;
    if reopened["claim_disposition"] != "reopened" {
        gaps.push(format!(
            "runtime MCP reconnect should reopen the same canonical install identity, got {}",
            reopened["claim_disposition"]
        ));
    }

    match read_resource_json_result(&reconnected_client, status_uri.clone()).await {
        Ok(status) => {
            if status["control_mode"] != "stopped" {
                gaps.push(
                    "runtime status resource must keep control_mode=stopped visible after reconnect"
                        .to_owned(),
                );
            }
            if status["autonomy_eligibility"] != "blocked" {
                gaps.push(
                    "runtime status resource must keep autonomy_eligibility=blocked visible after reconnect"
                        .to_owned(),
                );
            }
            if status["transition_source"] != "runtime_stop_control_mcp" {
                gaps.push(
                    "runtime status resource must keep transition_source visible after reconnect"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime status resource should stay readable after reconnect, got {error}"
        )),
    }

    match read_resource_json_result(&reconnected_client, failures_uri.clone()).await {
        Ok(failures) => {
            if failures["autonomy_eligibility"] != "blocked"
                || failures["transition_source"] != "runtime_stop_control_mcp"
            {
                gaps.push(
                    "runtime failures resource must preserve blocked eligibility and transition source after reconnect"
                        .to_owned(),
                );
            }
            if failures["last_rejection"]["code"] != "runtime_stopped" {
                gaps.push(
                    "runtime failures resource must preserve last_rejection.code after reconnect"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime failures resource should stay readable after reconnect, got {error}"
        )),
    }

    match render_prompt_result(&reconnected_client, install_id.clone()).await {
        Ok(prompt_text) => {
            if !prompt_text.contains("strategy_rebalance") {
                gaps.push(
                    "runtime control guidance prompt must keep the blocked attempted operation visible after reconnect"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime control guidance prompt should stay readable after reconnect, got {error}"
        )),
    }

    let clear = call_tool_json(
        &reconnected_client,
        TOOL_RUNTIME_CLEAR_STOP,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    if clear["control_mode"] != "active" || clear["autonomy_eligibility"] != "eligible" {
        gaps.push(
            "runtime.clear_stop must restore active eligible runtime control after reconnect"
                .to_owned(),
        );
    }

    match read_resource_json_result(&reconnected_client, failures_uri.clone()).await {
        Ok(failures) => {
            if failures["control_mode"] != "active"
                || failures["autonomy_eligibility"] != "eligible"
            {
                gaps.push(
                    "runtime failures resource must reflect active eligible state after clear_stop"
                        .to_owned(),
                );
            }
            if failures["last_rejection"]["code"] != "runtime_stopped"
                || failures["last_rejection"]["attempted_operation"] != "strategy_rebalance"
            {
                gaps.push(
                    "runtime failures resource must preserve last rejection diagnostics after clear_stop"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime failures resource should stay readable after clear_stop, got {error}"
        )),
    }

    match render_prompt_result(&reconnected_client, install_id.clone()).await {
        Ok(prompt_text) => {
            if !prompt_text.contains("Autonomy eligibility: eligible")
                || !prompt_text.contains("runtime_stopped")
            {
                gaps.push(
                    "runtime control guidance prompt must keep eligible recovery state and prior rejection diagnostics visible after clear_stop"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime control guidance prompt should stay readable after clear_stop, got {error}"
        )),
    }

    assert!(
        gaps.is_empty(),
        "S03 MCP runtime control contract missing canonical status/failure surfaces or reconnect-safe guidance: {}",
        gaps.join("; ")
    );

    reconnected_client
        .cancel()
        .await
        .expect("reconnected live stdio server should shut down cleanly");
}

async fn ready_path_harness() -> SkillBundleHarness {
    spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(READY_PATH_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(READY_PATH_OWNER_SETUP_MD),
        ),
    ])
    .await
}

async fn runtime_daemon_service(
    workspace_root: &Path,
) -> DaemonService<
    BaselinePolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
    NoopEvmAdapter,
> {
    let config = runtime_config(workspace_root);
    let harness = seeded_harness();
    DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        NoopEvmAdapter,
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    )
}

fn runtime_config(workspace_root: &Path) -> DaemonConfig {
    DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"))
}

fn seeded_harness() -> FakeHyperliquidTransport {
    let harness = FakeHyperliquidTransport::default();
    let open_orders = vec![HyperliquidOpenOrder {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        is_buy: false,
        price: "2412.7".to_owned(),
        size: "0.5".to_owned(),
        reduce_only: false,
        status: "resting".to_owned(),
        client_order_id: Some("hl-strategy-lp-1-1".to_owned()),
    }];
    harness.seed_open_orders(open_orders);
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: "filled".to_owned(),
        filled_size: "0.5".to_owned(),
    });
    harness.seed_user_fills(vec![HyperliquidUserFill {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "0.5".to_owned(),
        price: "2412.7".to_owned(),
        side: "sell".to_owned(),
        filled_at: "2026-03-12T00:00:11Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "-0.5".to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);
    harness
}

async fn register_strategy(
    service: &DaemonService<
        BaselinePolicy,
        SqliteReservationManager,
        RecordingSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
        NoopEvmAdapter,
    >,
) {
    let response: JsonRpcResponse<a2ex_daemon::StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
}

fn strategy_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-strategy-runtime-control-mcp",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-runtime-control-mcp",
            "request_kind": "strategy",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "strategy_id": "strategy-lp-1",
                "strategy_type": "stateful_hedge",
                "watchers": [{"watcher_type": "lp_position", "source": "local", "target": "TOKEN/USDT"}],
                "trigger_rules": [{"trigger_type": "drift_threshold", "metric": "delta_exposure_pct", "operator": ">", "value": "0.02", "cooldown_sec": 10}],
                "calculation_model": {"model_type": "delta_neutral_lp", "inputs": ["lp_token_balance"]},
                "action_templates": [{"action_type": "adjust_hedge", "venue": "hyperliquid", "instrument": "TOKEN-PERP", "target": "delta_neutral"}],
                "constraints": {"min_order_usd": 100, "max_slippage_bps": 40, "max_rebalances_per_hour": 60},
                "unwind_rules": [{"condition": "manual_stop"}]
            },
            "rationale": {"summary": "Keep delta neutral.", "main_risks": []},
            "execution_preferences": {"preview_only": false, "allow_fast_path": false}
        }),
    )
}

async fn spawn_live_client() -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root canonicalizes");
    let manifest_path = workspace_root.join("Cargo.toml");

    let mut command = tokio::process::Command::new(cargo);
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("-p")
        .arg("a2ex-mcp")
        .arg("--bin")
        .arg("a2ex-mcp")
        .current_dir(&workspace_root);

    let transport = TokioChildProcess::new(command).expect("live stdio server should spawn");
    let client = ()
        .serve(transport)
        .await
        .expect("rmcp client should initialize against the live stdio server");
    let server_info = client
        .peer_info()
        .expect("initialized server info should be available");
    assert_eq!(server_info.server_info.name, SERVER_NAME);
    client
}

async fn bootstrap_install_live(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_url: &str,
    workspace_root: &Path,
    expected_workspace_id: Option<String>,
    expected_install_id: Option<String>,
) -> Value {
    let mut arguments = json_map([
        ("install_url", Value::String(install_url.to_owned())),
        (
            "workspace_root",
            Value::String(workspace_root.display().to_string()),
        ),
    ]);
    if let Some(expected_workspace_id) = expected_workspace_id {
        arguments.insert(
            "expected_workspace_id".to_owned(),
            Value::String(expected_workspace_id),
        );
    }
    if let Some(expected_install_id) = expected_install_id {
        arguments.insert(
            "expected_install_id".to_owned(),
            Value::String(expected_install_id),
        );
    }
    call_tool_json(client, "onboarding.bootstrap_install", arguments).await
}

async fn call_tool_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool_name: &str,
    arguments: Map<String, Value>,
) -> Value {
    client
        .call_tool(CallToolRequestParams::new(tool_name.to_owned()).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("tool {tool_name} should succeed: {error}"))
        .structured_content
        .expect("tool result should include structured content")
}

async fn read_resource_json_result(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Result<Value, String> {
    let response = client
        .read_resource(ReadResourceRequestParams::new(uri.clone()))
        .await
        .map_err(|error| format!("resource {uri} read failed: {error}"))?;
    if response.contents.len() != 1 {
        return Err(format!(
            "resource {uri} should return exactly one payload, got {}",
            response.contents.len()
        ));
    }
    let text = match &response.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text,
        other => {
            return Err(format!(
                "resource {uri} should return text JSON, got {other:?}"
            ));
        }
    };
    serde_json::from_str(text).map_err(|error| format!("resource {uri} JSON parse failed: {error}"))
}

async fn render_prompt_result(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_id: String,
) -> Result<String, String> {
    let prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_RUNTIME_CONTROL_GUIDANCE).with_arguments(json_map(
                [(PROMPT_ARGUMENT_INSTALL_ID, Value::String(install_id))],
            )),
        )
        .await
        .map_err(|error| format!("prompt render failed: {error}"))?;
    Ok(prompt
        .messages
        .iter()
        .filter_map(|message| match &message.content {
            rmcp::model::PromptMessageContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn table_columns(connection: &Connection, table: &str) -> BTreeSet<String> {
    let mut statement = match connection.prepare(&format!("PRAGMA table_info({table})")) {
        Ok(statement) => statement,
        Err(_) => return BTreeSet::new(),
    };
    let rows = match statement.query_map([], |row| row.get::<_, String>(1)) {
        Ok(rows) => rows,
        Err(_) => return BTreeSet::new(),
    };
    rows.collect::<Result<BTreeSet<_>, _>>().unwrap_or_default()
}

fn require_columns(columns: &BTreeSet<String>, required: &[&str], gaps: &mut Vec<String>) {
    for column in required {
        if !columns.contains(*column) {
            gaps.push(format!(
                "runtime_control table is missing persisted column {column}"
            ));
        }
    }
}

fn ensure_contract_table_for_test_progression(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS runtime_control (
                scope_key TEXT PRIMARY KEY,
                control_mode TEXT NOT NULL,
                transition_reason TEXT NOT NULL,
                transition_source TEXT NOT NULL,
                transitioned_at TEXT NOT NULL,
                last_cleared_at TEXT,
                last_cleared_reason TEXT,
                last_cleared_source TEXT,
                last_rejection_code TEXT,
                last_rejection_message TEXT,
                last_rejection_operation TEXT,
                last_rejection_at TEXT,
                updated_at TEXT NOT NULL
            );",
        )
        .expect("runtime control contract table should be creatable for red-test progression");
}

fn upsert_runtime_control(connection: &Connection, row: RuntimeControlRow) {
    connection
        .execute(
            "INSERT INTO runtime_control (
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
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(scope_key) DO UPDATE SET
                control_mode = excluded.control_mode,
                transition_reason = excluded.transition_reason,
                transition_source = excluded.transition_source,
                transitioned_at = excluded.transitioned_at,
                last_cleared_at = excluded.last_cleared_at,
                last_cleared_reason = excluded.last_cleared_reason,
                last_cleared_source = excluded.last_cleared_source,
                last_rejection_code = excluded.last_rejection_code,
                last_rejection_message = excluded.last_rejection_message,
                last_rejection_operation = excluded.last_rejection_operation,
                last_rejection_at = excluded.last_rejection_at,
                updated_at = excluded.updated_at",
            params![
                RUNTIME_CONTROL_SCOPE,
                row.control_mode,
                row.transition_reason,
                row.transition_source,
                row.transitioned_at,
                row.last_cleared_at,
                row.last_cleared_reason,
                row.last_cleared_source,
                row.last_rejection_code,
                row.last_rejection_message,
                row.last_rejection_operation,
                row.last_rejection_at,
                row.updated_at,
            ],
        )
        .expect("runtime control contract row upserts");
}
