mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_hyperliquid_adapter::HyperliquidAdapter;
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_mcp::A2exSkillMcpServer;
use a2ex_policy::BaselinePolicy;
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
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
const TOOL_MATERIALIZE_STRATEGY_SELECTION: &str = "strategy_selection.materialize";
const TOOL_APPROVE_STRATEGY_SELECTION: &str = "strategy_selection.approve";
const TOOL_APPLY_STRATEGY_OVERRIDE: &str = "strategy_selection.apply_override";
const TOOL_RUNTIME_STOP: &str = "runtime.stop";

const RESOURCE_STRATEGY_RUNTIME_ELIGIBILITY_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/eligibility";
const RESOURCE_STRATEGY_RUNTIME_MONITORING_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/monitoring";
const PROMPT_STRATEGY_SELECTION_GUIDANCE: &str = "strategy_selection.guidance";
const PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";
const PROMPT_ARGUMENT_PROPOSAL_ID: &str = "proposal_id";
const PROMPT_ARGUMENT_SELECTION_ID: &str = "selection_id";

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
async fn strategy_runtime_handoff_mcp_surface_advertises_read_first_eligibility_and_monitoring_resources()
 {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S02 runtime-handoff handlers exist");

    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_STRATEGY_RUNTIME_ELIGIBILITY_TEMPLATE),
        "S02 must advertise a stable approved-runtime eligibility resource keyed by install/proposal/selection identity"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_STRATEGY_RUNTIME_MONITORING_TEMPLATE),
        "S02 must advertise a stable approved-runtime monitoring resource instead of relying on approval receipts or session memory"
    );
}

#[tokio::test]
async fn strategy_runtime_handoff_mcp_flow_requires_thin_approval_reconnect_safe_resources_and_runtime_hold_reporting()
 {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S02 runtime-handoff handlers exist");
    let advertised_resources = capabilities
        .resources
        .iter()
        .map(|resource| resource.uri_template.as_str())
        .collect::<Vec<_>>();

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
    let onboarding_ready = apply_action(
        &first_client,
        &install_id,
        json!({
            "kind": "resolve_owner_decision",
            "step_key": "approve-max-spread-budget",
            "resolution": "approved"
        }),
    )
    .await;
    assert_eq!(onboarding_ready["aggregate_status"], "ready");

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

    let routing_service = routed_daemon_service(workspace_root.path()).await;
    let submit = routing_service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request("req-runtime-handoff-mcp-1")
        .await
        .expect("preview builds");
    let route_id = expected_route_id(&preview);

    let first_eval = call_tool_json(
        &first_client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-runtime-handoff-mcp-1".to_owned()),
            ),
        ]),
    )
    .await;
    assert_eq!(
        first_eval["identity"]["request_id"],
        "req-runtime-handoff-mcp-1"
    );

    let reservations =
        SqliteReservationManager::open(workspace_root.path().join(".a2ex-daemon/state.db"))
            .await
            .expect("reservations open");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-runtime-handoff-mcp-1".to_owned(),
            execution_id: "req-runtime-handoff-mcp-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("reservation evidence persists");

    let second_eval = call_tool_json(
        &first_client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            (
                "request_id",
                Value::String("req-runtime-handoff-mcp-1".to_owned()),
            ),
        ]),
    )
    .await;
    let approval_entry = if second_eval["current_step_key"] == "satisfy_route_approvals" {
        second_eval
    } else {
        call_tool_json(
            &first_client,
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
        .await
    };
    assert_eq!(
        approval_entry["current_step_key"],
        "satisfy_route_approvals"
    );

    let ready_route = call_tool_json(
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
    assert_eq!(ready_route["status"], "ready");

    let materialized = call_tool_json(
        &first_client,
        TOOL_MATERIALIZE_STRATEGY_SELECTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
        ]),
    )
    .await;
    let selection_id = materialized["selection_id"]
        .as_str()
        .expect("selection id string")
        .to_owned();

    let mut gaps = Vec::new();
    for required in [
        RESOURCE_STRATEGY_RUNTIME_ELIGIBILITY_TEMPLATE,
        RESOURCE_STRATEGY_RUNTIME_MONITORING_TEMPLATE,
    ] {
        if !advertised_resources.contains(&required) {
            gaps.push(format!(
                "initialize() missing advertised approved-runtime resource template {required}"
            ));
        }
    }

    if gaps.is_empty() {
        let approved = call_tool_json(
            &first_client,
            TOOL_APPROVE_STRATEGY_SELECTION,
            json_map([
                ("install_id", Value::String(install_id.clone())),
                ("proposal_id", Value::String(proposal_id.clone())),
                ("selection_id", Value::String(selection_id.clone())),
                ("expected_selection_revision", Value::from(1)),
                (
                    "approval",
                    json!({
                        "approved_by": "owner",
                        "note": "approve through the shipped MCP seam"
                    }),
                ),
            ]),
        )
        .await;

        let eligibility_uri = format!(
            "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/eligibility"
        );
        let monitoring_uri = format!(
            "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/monitoring"
        );

        if approved["eligibility_uri"] != eligibility_uri {
            gaps.push(
                "strategy_selection.approve must return the canonical eligibility_uri instead of forcing agents to infer or replay approval state"
                    .to_owned(),
            );
        }
        if approved["monitoring_uri"] != monitoring_uri {
            gaps.push(
                "strategy_selection.approve must return the canonical monitoring_uri instead of embedding session-only runtime guesses"
                    .to_owned(),
            );
        }
        if approved.get("proposal_snapshot").is_some() {
            gaps.push(
                "strategy_selection.approve should stay a thin mutation receipt for S02 and rely on read-first eligibility/monitoring resources for rich state"
                    .to_owned(),
            );
        }

        let eligibility_before_stop =
            read_resource_json(&first_client, eligibility_uri.clone()).await;
        if eligibility_before_stop["install_id"] != install_id
            || eligibility_before_stop["proposal_id"] != proposal_id
            || eligibility_before_stop["selection_id"] != selection_id
        {
            gaps.push(
                "eligibility resource must retain canonical install/proposal/selection identity"
                    .to_owned(),
            );
        }
        if eligibility_before_stop["route_id"] != route_id {
            gaps.push("eligibility resource must link approval to the ready route_id".to_owned());
        }
        if eligibility_before_stop["request_id"] != "req-runtime-handoff-mcp-1" {
            gaps.push("eligibility resource must link approval to the route request_id".to_owned());
        }
        if eligibility_before_stop["eligibility_status"] != "eligible" {
            gaps.push(
                "eligibility resource must derive eligibility_status=eligible from fresh readiness plus active runtime control"
                    .to_owned(),
            );
        }
        if eligibility_before_stop["runtime_control_mode"] != "active" {
            gaps.push(
                "eligibility resource must expose runtime_control_mode=active before a stop is applied"
                    .to_owned(),
            );
        }
        if !eligibility_before_stop["hold_reason"].is_null() {
            gaps.push(
                "eligible approved-runtime resource must not invent a hold_reason while active"
                    .to_owned(),
            );
        }

        let monitoring_before_stop =
            read_resource_json(&first_client, monitoring_uri.clone()).await;
        if monitoring_before_stop["eligibility_uri"] != eligibility_uri
            || monitoring_before_stop["monitoring_uri"] != monitoring_uri
        {
            gaps.push(
                "monitoring resource must cross-link its canonical eligibility and monitoring uris"
                    .to_owned(),
            );
        }
        if monitoring_before_stop.get("last_runtime_failure").is_none() {
            gaps.push(
                "monitoring resource must expose a stable last_runtime_failure field even when null so failure truth is inspectable"
                    .to_owned(),
            );
        }
        if monitoring_before_stop
            .get("last_runtime_rejection")
            .is_none()
        {
            gaps.push(
                "monitoring resource must expose a stable last_runtime_rejection field even when null so diagnostics stay truthful"
                    .to_owned(),
            );
        }

        let guidance_prompt = first_client
            .get_prompt(
                GetPromptRequestParams::new(PROMPT_STRATEGY_SELECTION_GUIDANCE).with_arguments(
                    json_map([
                        (
                            PROMPT_ARGUMENT_INSTALL_ID,
                            Value::String(install_id.clone()),
                        ),
                        (
                            PROMPT_ARGUMENT_PROPOSAL_ID,
                            Value::String(proposal_id.clone()),
                        ),
                        (
                            PROMPT_ARGUMENT_SELECTION_ID,
                            Value::String(selection_id.clone()),
                        ),
                    ]),
                ),
            )
            .await
            .expect("strategy-selection guidance prompt should render from canonical resources");
        let guidance_text = prompt_text(&guidance_prompt);
        if !guidance_text.contains(&eligibility_uri) || !guidance_text.contains(&monitoring_uri) {
            gaps.push(
                "strategy_selection.guidance must point future agents at approved-runtime eligibility and monitoring resources"
                    .to_owned(),
            );
        }
        if !guidance_text.contains("Do not rely on prior mutation receipts or session memory") {
            gaps.push(
                "strategy_selection.guidance must keep rejecting session-memory shortcuts after approval"
                    .to_owned(),
            );
        }

        let stopped = call_tool_json(
            &first_client,
            TOOL_RUNTIME_STOP,
            json_map([("install_id", Value::String(install_id.clone()))]),
        )
        .await;
        if stopped["control_mode"] != "stopped" {
            gaps.push("runtime.stop must succeed before hold reporting is asserted".to_owned());
        }

        let monitoring_after_stop = read_resource_json(&first_client, monitoring_uri.clone()).await;
        if monitoring_after_stop["hold_reason"] != "runtime_control_stopped" {
            gaps.push(
                "monitoring resource must report runtime_control_stopped as the hold_reason when runtime.stop is active"
                    .to_owned(),
            );
        }
        if monitoring_after_stop["runtime_control"]["control_mode"] != "stopped" {
            gaps.push(
                "monitoring resource must expose the canonical runtime_control block state"
                    .to_owned(),
            );
        }
        if !monitoring_after_stop["last_runtime_failure"].is_null()
            && monitoring_after_stop["last_runtime_failure"]["code"] == "runtime_stopped"
        {
            gaps.push(
                "monitoring resource must keep last_runtime_failure truthful and separate from runtime-control hold state"
                    .to_owned(),
            );
        }

        let overridden = call_tool_json(
            &first_client,
            TOOL_APPLY_STRATEGY_OVERRIDE,
            json_map([
                ("install_id", Value::String(install_id.clone())),
                ("proposal_id", Value::String(proposal_id.clone())),
                ("selection_id", Value::String(selection_id.clone())),
                (
                    "override",
                    json!({
                        "key": "approve-max-spread-budget",
                        "value": { "resolution": "approved", "budget_bps": 25 },
                        "rationale": "change a readiness-sensitive assumption after approval"
                    }),
                ),
            ]),
        )
        .await;
        if overridden["summary"]["selection_revision"] != 2 {
            gaps.push(
                "first readiness-sensitive override after approval must advance selection_revision to 2"
                    .to_owned(),
            );
        }

        let eligibility_after_override =
            read_resource_json(&first_client, eligibility_uri.clone()).await;
        if eligibility_after_override["eligibility_status"] != "blocked" {
            gaps.push(
                "eligibility resource must block autonomy after a readiness-sensitive override until readiness is reevaluated"
                    .to_owned(),
            );
        }
        if eligibility_after_override["hold_reason"] != "route_readiness_stale" {
            gaps.push(
                "eligibility resource must expose route_readiness_stale as the blocked-path diagnostic after override invalidation"
                    .to_owned(),
            );
        }

        first_client
            .cancel()
            .await
            .expect("first live stdio server should shut down cleanly");

        let reconnected_client = spawn_live_client().await;
        let pre_reopen_eligibility_error =
            read_resource_error(&reconnected_client, eligibility_uri.clone()).await;
        if !(pre_reopen_eligibility_error.contains("install")
            || pre_reopen_eligibility_error.contains("locator"))
        {
            gaps.push(format!(
                "approved-runtime eligibility resource must reject reconnect reads before bootstrap reopen repopulates the install locator, got {pre_reopen_eligibility_error}"
            ));
        }
        let pre_reopen_monitoring_error =
            read_resource_error(&reconnected_client, monitoring_uri.clone()).await;
        if !(pre_reopen_monitoring_error.contains("install")
            || pre_reopen_monitoring_error.contains("locator"))
        {
            gaps.push(format!(
                "approved-runtime monitoring resource must reject reconnect reads before bootstrap reopen repopulates the install locator, got {pre_reopen_monitoring_error}"
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
        if reopened["claim_disposition"] != "reopened" {
            gaps.push(
                "bootstrap reopen must preserve install identity for reconnect-safe approved-runtime resources"
                    .to_owned(),
            );
        }

        let eligibility_after_restart =
            read_resource_json(&reconnected_client, eligibility_uri.clone()).await;
        let monitoring_after_restart =
            read_resource_json(&reconnected_client, monitoring_uri.clone()).await;
        if eligibility_after_restart["hold_reason"] != "route_readiness_stale" {
            gaps.push(
                "eligibility resource must preserve stale-readiness block diagnostics after reconnect"
                    .to_owned(),
            );
        }
        if monitoring_after_restart["runtime_control"]["control_mode"] != "stopped" {
            gaps.push(
                "monitoring resource must preserve runtime-control stopped state after reconnect"
                    .to_owned(),
            );
        }

        reconnected_client
            .cancel()
            .await
            .expect("reconnected live stdio server should shut down cleanly");
    } else {
        first_client
            .cancel()
            .await
            .expect("live stdio server should shut down cleanly when capability assertions fail");
    }

    assert!(
        gaps.is_empty(),
        "S02 MCP runtime-handoff contract missing thin approval receipts, canonical eligibility/monitoring resources, or reconnect-safe hold reporting: {}",
        gaps.join("; ")
    );
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
    JsonRpcRequest::new(
        "req-runtime-handoff-mcp-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-runtime-handoff-mcp-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-runtime-handoff-mcp-1",
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
