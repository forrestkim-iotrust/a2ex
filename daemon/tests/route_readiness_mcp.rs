mod support;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_mcp::A2exSkillMcpServer;
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
};
use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams, ResourceContents,
    },
    transport::TokioChildProcess,
};
use serde_json::{Map, Value, json};
use support::{
    across_harness::FakeAcrossTransport,
    hyperliquid_harness::FakeHyperliquidTransport,
    prediction_market_harness::FakePredictionMarketTransport,
    skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle},
};
use tempfile::tempdir;

use a2ex_across_adapter::AcrossAdapter;
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;

const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";
const TOOL_EVALUATE_ROUTE_READINESS: &str = "readiness.evaluate_route";

const RESOURCE_ROUTE_READINESS_TEMPLATE: &str =
    "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/summary";
const RESOURCE_ROUTE_BLOCKERS_TEMPLATE: &str =
    "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/blockers";
const PROMPT_ROUTE_READINESS_GUIDANCE: &str = "readiness.route_guidance";
const PROMPT_ROUTE_BLOCKER_SUMMARY: &str = "readiness.route_blocker_summary";
const PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";
const PROMPT_ARGUMENT_PROPOSAL_ID: &str = "proposal_id";
const PROMPT_ARGUMENT_ROUTE_ID: &str = "route_id";

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
struct RecordingSigner {
    handoffs: Arc<Mutex<Vec<String>>>,
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, request: &ExecutionRequest) {
        self.handoffs
            .lock()
            .expect("handoff lock")
            .push(request.action_kind.clone());
    }
}

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

#[tokio::test]
async fn route_readiness_mcp_surface_advertises_tool_resources_and_prompts() {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before route-readiness handlers exist");

    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == TOOL_EVALUATE_ROUTE_READINESS),
        "S01/T04 must advertise a canonical route-readiness evaluation tool instead of forcing agents to reconstruct readiness by replaying preview or prompt prose"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_ROUTE_READINESS_TEMPLATE),
        "S01/T04 must advertise a stable route-readiness summary resource keyed to install/proposal/route identity"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_ROUTE_BLOCKERS_TEMPLATE),
        "blocked or incomplete route readiness must stay inspectable through a dedicated blockers resource instead of disappearing into tool errors"
    );
    assert!(
        capabilities
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_ROUTE_READINESS_GUIDANCE),
        "S01/T04 must ship a route-readiness guidance prompt that points future agents at the canonical resources"
    );
    assert!(
        capabilities
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_ROUTE_BLOCKER_SUMMARY),
        "S01/T04 must ship a blocker-summary prompt for blocked or incomplete readiness"
    );
}

#[tokio::test]
async fn route_readiness_mcp_flow_preserves_blocked_state_visibility_after_evaluation() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let bootstrap = bootstrap_install_live(&client, &entry_url, workspace_root.path()).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();
    apply_action(
        &client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let ready = apply_action(
        &client,
        &install_id,
        json!({
            "kind": "resolve_owner_decision",
            "step_key": "approve-max-spread-budget",
            "resolution": "approved"
        }),
    )
    .await;
    assert_eq!(ready["aggregate_status"], "ready");

    let load = call_tool_json(
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.clone()))]),
    )
    .await;
    let proposal_id = load["session_id"]
        .as_str()
        .expect("session id string")
        .to_owned();
    let proposal = call_tool_json(
        &client,
        TOOL_GENERATE_PROPOSAL_PACKET,
        json_map([("session_id", Value::String(proposal_id.clone()))]),
    )
    .await;
    assert_eq!(proposal["proposal_readiness"], "ready");

    let service = routed_daemon_service(workspace_root.path()).await;
    let submit = service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = service
        .preview_intent_request("req-route-readiness-1")
        .await
        .expect("preview builds");
    let route_id = expected_route_id(&preview);

    let evaluation = call_tool_json(
        &client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-route-readiness-1".to_owned()),
            ),
        ]),
    )
    .await;

    assert_eq!(evaluation["identity"]["install_id"], install_id);
    assert_eq!(evaluation["identity"]["proposal_id"], proposal_id);
    assert_eq!(evaluation["identity"]["route_id"], route_id);
    assert_eq!(evaluation["status"], "incomplete");
    assert_eq!(evaluation["capital"]["required_capital_usd"], 3000);
    assert_eq!(evaluation["capital"]["completeness"], "unknown");
    assert!(
        evaluation["approvals"]
            .as_array()
            .is_some_and(|tuples| tuples.iter().any(|tuple| {
                tuple["venue"] == "across"
                    && tuple["approval_type"] == "erc20_allowance"
                    && tuple["asset"] == "USDC"
                    && tuple["chain"] == "base"
            })),
        "route readiness MCP must expose typed venue-specific approval tuples rather than a collapsed approved boolean"
    );
    assert_eq!(
        evaluation["recommended_owner_action"]["kind"],
        "fund_route_capital"
    );
    assert!(
        evaluation["blockers"]
            .as_array()
            .is_some_and(|blockers| blockers
                .iter()
                .any(|blocker| blocker["code"] == "capital_evidence_incomplete")),
        "route-readiness evaluation must leave the blocked or incomplete state inspectable in the returned summary"
    );

    let summary_uri =
        format!("a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/summary");
    let blockers_uri =
        format!("a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/blockers");
    let summary = read_resource_json(&client, summary_uri.clone()).await;
    let blockers = read_resource_json(&client, blockers_uri.clone()).await;
    assert_eq!(summary["identity"]["install_id"], install_id);
    assert_eq!(summary["capital"]["completeness"], "unknown");
    assert!(
        blockers["blockers"].as_array().is_some_and(|items| items
            .iter()
            .any(|item| item["code"] == "capital_evidence_incomplete")),
        "blocked-state visibility must survive beyond the tool call through a stable resource"
    );

    let guidance_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_ROUTE_READINESS_GUIDANCE).with_arguments(json_map(
                [
                    (
                        PROMPT_ARGUMENT_INSTALL_ID,
                        Value::String(install_id.clone()),
                    ),
                    (
                        PROMPT_ARGUMENT_PROPOSAL_ID,
                        Value::String(proposal_id.clone()),
                    ),
                    (PROMPT_ARGUMENT_ROUTE_ID, Value::String(route_id.clone())),
                ],
            )),
        )
        .await
        .expect("route-readiness guidance prompt should render from canonical resources");
    let guidance_text = prompt_text(&guidance_prompt);
    assert!(guidance_text.contains(&summary_uri));
    assert!(guidance_text.contains("fund_route_capital"));

    let blocker_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_ROUTE_BLOCKER_SUMMARY).with_arguments(json_map([
                (PROMPT_ARGUMENT_INSTALL_ID, Value::String(install_id)),
                (PROMPT_ARGUMENT_PROPOSAL_ID, Value::String(proposal_id)),
                (PROMPT_ARGUMENT_ROUTE_ID, Value::String(route_id)),
            ])),
        )
        .await
        .expect("route blocker prompt should render from canonical resources");
    let blocker_text = prompt_text(&blocker_prompt);
    assert!(blocker_text.contains(&blockers_uri));
    assert!(blocker_text.contains("capital_evidence_incomplete"));
    assert!(blocker_text.contains("unknown"));
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

async fn routed_daemon_service(
    workspace_root: &Path,
) -> DaemonService<
    BaselinePolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
    a2ex_evm_adapter::NoopEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    let across = FakeAcrossTransport::default();
    let prediction = FakePredictionMarketTransport::default();
    let hyperliquid = FakeHyperliquidTransport::default();
    DaemonService::from_config_with_multi_venue_adapters(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        a2ex_evm_adapter::NoopEvmAdapter,
        AcrossAdapter::with_transport(across.transport(), 0),
        PredictionMarketAdapter::with_transport(prediction.transport()),
        HyperliquidAdapter::with_transport(hyperliquid.transport(), 0),
    )
}

fn expected_route_id(preview: &a2ex_daemon::IntentPreviewResponse) -> String {
    let adapters = preview
        .plan_preview
        .as_ref()
        .map(|plan| {
            plan.steps
                .iter()
                .map(|step| step.adapter.as_str())
                .collect::<Vec<_>>()
                .join(":")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "no_steps".to_owned());
    format!("{:?}:{adapters}", preview.route.route).to_lowercase()
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
    ().serve(transport)
        .await
        .expect("rmcp client should initialize against the live stdio server")
}

async fn bootstrap_install_live(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_url: &str,
    workspace_root: &Path,
) -> Value {
    call_tool_json(
        client,
        TOOL_BOOTSTRAP_INSTALL,
        json_map([
            ("install_url", Value::String(install_url.to_owned())),
            (
                "workspace_root",
                Value::String(workspace_root.display().to_string()),
            ),
        ]),
    )
    .await
}

async fn apply_action(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_id: &str,
    action: Value,
) -> Value {
    call_tool_json(
        client,
        TOOL_APPLY_ONBOARDING_ACTION,
        json_map([
            ("install_id", Value::String(install_id.to_owned())),
            ("action", action),
        ]),
    )
    .await
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

async fn read_resource_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Value {
    let response = client
        .read_resource(ReadResourceRequestParams::new(uri.clone()))
        .await
        .unwrap_or_else(|error| panic!("resource {uri} should be readable: {error}"));
    assert_eq!(response.contents.len(), 1);
    let text = match &response.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text,
        other => panic!("expected text resource contents, got {other:?}"),
    };
    serde_json::from_str(text).expect("resource contents should be valid JSON")
}

fn prompt_text(prompt: &rmcp::model::GetPromptResult) -> String {
    prompt
        .messages
        .iter()
        .filter_map(|message| match &message.content {
            rmcp::model::PromptMessageContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-route-readiness-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-route-readiness-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-route-readiness-1",
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "us-election-2028",
                    "side": "yes",
                    "target_notional_usd": 3000
                },
                "constraints": {
                    "allowed_venues": ["polymarket", "kalshi"],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "normal",
                    "hedge_ratio_bps": 4000
                },
                "funding": {
                    "preferred_asset": "USDC",
                    "source_chain": "base"
                },
                "post_actions": [
                    {"action_type": "hedge", "venue": "hyperliquid"}
                ]
            },
            "rationale": {"summary": "bridge then enter and hedge", "main_risks": ["bridge delay"]},
            "execution_preferences": {"preview_only": false, "allow_fast_path": true}
        }),
    )
}
