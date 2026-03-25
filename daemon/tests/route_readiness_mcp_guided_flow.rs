mod support;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_mcp::A2exSkillMcpServer;
use a2ex_policy::BaselinePolicy;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
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

const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";
const TOOL_EVALUATE_ROUTE_READINESS: &str = "readiness.evaluate_route";
const TOOL_APPLY_ROUTE_READINESS_ACTION: &str = "readiness.apply_action";

const RESOURCE_ROUTE_SUMMARY_TEMPLATE: &str =
    "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/summary";
const RESOURCE_ROUTE_PROGRESS_TEMPLATE: &str =
    "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/progress";
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

#[tokio::test]
async fn route_readiness_guided_mcp_surface_advertises_separate_action_and_progress_contracts() {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S02 route-guidance handlers exist");

    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == TOOL_EVALUATE_ROUTE_READINESS),
        "S02 must keep advertising the evaluate tool as the canonical route-truth refresh surface"
    );
    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == TOOL_APPLY_ROUTE_READINESS_ACTION),
        "S02 must advertise a separate readiness.apply_action tool so owner progress mutation is distinct from readiness.evaluate_route"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_ROUTE_SUMMARY_TEMPLATE),
        "S02 must keep advertising a stable route summary resource keyed by install/proposal/route identity"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_ROUTE_PROGRESS_TEMPLATE),
        "S02 must advertise a dedicated route progress resource instead of overloading evaluate tool output"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_ROUTE_BLOCKERS_TEMPLATE),
        "S02 must keep advertising a blockers resource for route-readiness failures"
    );
    assert!(
        capabilities
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_ROUTE_READINESS_GUIDANCE),
        "S02 must keep shipping a route guidance prompt for canonical route-readiness state"
    );
    assert!(
        capabilities
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_ROUTE_BLOCKER_SUMMARY),
        "S02 must keep shipping a blocker summary prompt for route-readiness failures"
    );
}

#[tokio::test]
async fn route_readiness_guided_mcp_flow_requires_progress_resources_and_reconnect_safe_state() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");

    let first_client = spawn_live_client().await;
    let bootstrap =
        bootstrap_install_live(&first_client, &entry_url, workspace_root.path(), None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id string")
        .to_owned();
    apply_action(
        &first_client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let ready = apply_action(
        &first_client,
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
        &first_client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.clone()))]),
    )
    .await;
    let proposal_id = load["session_id"]
        .as_str()
        .expect("session id string")
        .to_owned();
    let proposal = call_tool_json(
        &first_client,
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
        &first_client,
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
    assert_eq!(evaluation["status"], "incomplete");

    let summary_uri =
        format!("a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/summary");
    let progress_uri =
        format!("a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/progress");
    let blockers_uri =
        format!("a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/blockers");

    let rejection_error = call_tool_error(
        &first_client,
        TOOL_APPLY_ROUTE_READINESS_ACTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "action",
                json!({ "kind": "complete_step", "step_key": "satisfy_route_approvals" }),
            ),
        ]),
    )
    .await;

    let summary_before_restart = read_resource_json(&first_client, summary_uri.clone()).await;
    let progress_before_restart = read_resource_json(&first_client, progress_uri.clone()).await;
    let blockers_before_restart = read_resource_json(&first_client, blockers_uri.clone()).await;

    let guidance_prompt = first_client
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
        .expect("route guidance prompt should render from canonical route resources");
    let guidance_text = prompt_text(&guidance_prompt);

    first_client
        .cancel()
        .await
        .expect("first live stdio server should shut down cleanly");

    let mut gaps = Vec::new();
    let reconnected_client = spawn_live_client().await;
    let pre_reopen_summary_error =
        read_resource_error(&reconnected_client, summary_uri.clone()).await;
    let pre_reopen_progress_error =
        read_resource_error(&reconnected_client, progress_uri.clone()).await;
    if !(pre_reopen_summary_error.contains("install")
        || pre_reopen_summary_error.contains("locator"))
    {
        gaps.push(format!(
            "route summary resource must reject reconnect reads before onboarding.bootstrap_install repopulates the install locator, got {pre_reopen_summary_error}"
        ));
    }
    if !(pre_reopen_progress_error.contains("install")
        || pre_reopen_progress_error.contains("locator"))
    {
        gaps.push(format!(
            "route progress resource must reject reconnect reads before onboarding.bootstrap_install repopulates the install locator, got {pre_reopen_progress_error}"
        ));
    }
    let reopened = bootstrap_install_live(
        &reconnected_client,
        &entry_url,
        workspace_root.path(),
        Some(workspace_id),
        Some(install_id.clone()),
    )
    .await;
    assert_eq!(reopened["claim_disposition"], "reopened");

    let summary_after_restart = read_resource_json(&reconnected_client, summary_uri.clone()).await;
    let progress_after_restart =
        read_resource_json(&reconnected_client, progress_uri.clone()).await;
    let blockers_after_restart =
        read_resource_json(&reconnected_client, blockers_uri.clone()).await;

    let fund_result = call_tool_json(
        &reconnected_client,
        TOOL_APPLY_ROUTE_READINESS_ACTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "action",
                json!({ "kind": "complete_step", "step_key": "fund_route_capital" }),
            ),
        ]),
    )
    .await;
    assert_eq!(fund_result["current_step_key"], "satisfy_route_approvals");

    let submit = service
        .submit_intent(intent_request_with_id("req-route-readiness-2"))
        .await
        .expect("submit second intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let second_preview = service
        .preview_intent_request("req-route-readiness-2")
        .await
        .expect("second preview builds");
    assert_eq!(expected_route_id(&second_preview), route_id);

    let stale_evaluation = call_tool_json(
        &reconnected_client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-route-readiness-2".to_owned()),
            ),
        ]),
    )
    .await;
    let progress_after_stale = read_resource_json(&reconnected_client, progress_uri.clone()).await;

    let blocker_prompt = reconnected_client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_ROUTE_BLOCKER_SUMMARY).with_arguments(json_map([
                (
                    PROMPT_ARGUMENT_INSTALL_ID,
                    Value::String(install_id.clone()),
                ),
                (
                    PROMPT_ARGUMENT_PROPOSAL_ID,
                    Value::String(proposal_id.clone()),
                ),
                (PROMPT_ARGUMENT_ROUTE_ID, Value::String(route_id.clone())),
            ])),
        )
        .await
        .expect("route blocker prompt should render after reconnect");
    let blocker_text = prompt_text(&blocker_prompt);

    collect_guided_route_json_gaps("evaluation", &evaluation, &mut gaps);
    collect_guided_route_json_gaps("summary_before_restart", &summary_before_restart, &mut gaps);
    collect_guided_route_json_gaps(
        "progress_before_restart",
        &progress_before_restart,
        &mut gaps,
    );
    collect_guided_route_json_gaps("summary_after_restart", &summary_after_restart, &mut gaps);
    collect_guided_route_json_gaps("progress_after_restart", &progress_after_restart, &mut gaps);
    if !rejection_error.contains("out_of_order_action") {
        gaps.push("route readiness apply_action rejection must surface out_of_order_action through the MCP error".to_owned());
    }
    if progress_before_restart["last_rejection"]["code"] != "out_of_order_action" {
        gaps.push("route progress resource must persist last_rejection.code=out_of_order_action before reconnect".to_owned());
    }
    if progress_after_restart["last_rejection"]["code"] != "out_of_order_action" {
        gaps.push(
            "route progress resource must preserve last_rejection.code after reconnect".to_owned(),
        );
    }
    if blockers_before_restart["last_rejection"]["code"] != "out_of_order_action"
        || blockers_after_restart["last_rejection"]["code"] != "out_of_order_action"
    {
        gaps.push(
            "route blockers resource must preserve last_rejection diagnostics across reconnect"
                .to_owned(),
        );
    }
    if !blockers_before_restart["blockers"]
        .as_array()
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item["code"] == "capital_evidence_incomplete")
        })
    {
        gaps.push(
            "route blockers resource must expose capital_evidence_incomplete before reconnect"
                .to_owned(),
        );
    }
    if !guidance_text.contains(TOOL_APPLY_ROUTE_READINESS_ACTION) {
        gaps.push(format!(
            "route guidance prompt must reference {} so evaluate vs apply semantics stay explicit",
            TOOL_APPLY_ROUTE_READINESS_ACTION
        ));
    }
    if !guidance_text.contains(&progress_uri) {
        gaps.push(format!(
            "route guidance prompt must reference canonical progress resource {}",
            progress_uri
        ));
    }
    if !guidance_text.contains(&summary_uri) || !guidance_text.contains(&blockers_uri) {
        gaps.push(
            "route guidance prompt must point agents at canonical summary and blockers URIs"
                .to_owned(),
        );
    }
    if stale_evaluation["stale"]["status"] != "stale"
        || progress_after_stale["stale"]["status"] != "stale"
    {
        gaps.push("route readiness reevaluation drift must mark stale state through both tool output and progress resource".to_owned());
    }
    let review_step_after_stale =
        progress_after_stale["ordered_steps"]
            .as_array()
            .and_then(|steps| {
                steps.iter().find(|step| {
                    step["step_key"] == "review_stale_readiness" && step["status"] == "pending"
                })
            });
    if review_step_after_stale.is_none() {
        gaps.push(
            "route progress resource must expose a pending review_stale_readiness step after drift"
                .to_owned(),
        );
    }
    if !blocker_text.contains(&blockers_uri) {
        gaps.push(format!(
            "route blocker prompt must reference blockers resource {} after reconnect",
            blockers_uri
        ));
    }
    if !blocker_text.contains("capital_evidence_incomplete") {
        gaps.push(
            "route blocker prompt must preserve canonical blocker codes after reconnect".to_owned(),
        );
    }
    if !blocker_text.contains("last_rejection") {
        gaps.push("route blocker prompt must mention durable last_rejection diagnostics for rejected route actions".to_owned());
    }
    if !blocker_text.contains("stale") {
        gaps.push("route blocker prompt must mention stale readiness review when reevaluation drift invalidates progress".to_owned());
    }

    assert!(
        gaps.is_empty(),
        "S02 MCP route-readiness contract missing action/progress/reconnect visibility: {}",
        gaps.join("; ")
    );

    reconnected_client
        .cancel()
        .await
        .expect("reconnected live stdio server should shut down cleanly");
}

fn collect_guided_route_json_gaps(label: &str, value: &Value, gaps: &mut Vec<String>) {
    if value
        .get("ordered_steps")
        .and_then(Value::as_array)
        .is_none()
    {
        gaps.push(format!(
            "{label} missing ordered_steps for route-scoped progress guidance"
        ));
    }
    if value.get("current_step_key").and_then(Value::as_str) != Some("fund_route_capital") {
        gaps.push(format!(
            "{label} missing current_step_key=fund_route_capital"
        ));
    }
    if value.get("last_rejection").is_none() {
        gaps.push(format!("{label} missing durable last_rejection visibility"));
    }
    if value
        .get("evaluation")
        .and_then(|evaluation| evaluation.get("fingerprint"))
        .and_then(Value::as_str)
        .is_none()
    {
        gaps.push(format!(
            "{label} missing evaluation fingerprint for stale invalidation"
        ));
    }
    if value
        .get("stale")
        .and_then(|stale| stale.get("status"))
        .and_then(Value::as_str)
        .is_none()
    {
        gaps.push(format!("{label} missing stale readiness state"));
    }
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
    call_tool_json(client, TOOL_BOOTSTRAP_INSTALL, arguments).await
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

async fn call_tool_error(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool_name: &str,
    arguments: Map<String, Value>,
) -> String {
    match client
        .call_tool(CallToolRequestParams::new(tool_name.to_owned()).with_arguments(arguments))
        .await
    {
        Ok(response) => panic!(
            "tool {tool_name} should have failed, got structured response {:?}",
            response.structured_content
        ),
        Err(error) => error.to_string(),
    }
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

async fn read_resource_error(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> String {
    match client
        .read_resource(ReadResourceRequestParams::new(uri.clone()))
        .await
    {
        Ok(response) => panic!(
            "resource {uri} should have failed before reopen, got {:?}",
            response.contents
        ),
        Err(error) => error.to_string(),
    }
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
    intent_request_with_id("req-route-readiness-1")
}

fn intent_request_with_id(request_id: &str) -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        request_id,
        "daemon.submitIntent",
        json!({
            "request_id": request_id,
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": format!("intent-{request_id}"),
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
