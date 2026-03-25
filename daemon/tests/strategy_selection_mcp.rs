mod support;

use std::path::{Path, PathBuf};

use a2ex_mcp::A2exSkillMcpServer;
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams, ResourceContents,
    },
    transport::TokioChildProcess,
};
use serde_json::{Map, Value, json};
use support::skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle};
use tempfile::tempdir;

const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";

const TOOL_MATERIALIZE_STRATEGY_SELECTION: &str = "strategy_selection.materialize";
const TOOL_APPLY_STRATEGY_OVERRIDE: &str = "strategy_selection.apply_override";
const TOOL_APPROVE_STRATEGY_SELECTION: &str = "strategy_selection.approve";

const RESOURCE_STRATEGY_SELECTION_SUMMARY_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/summary";
const RESOURCE_STRATEGY_SELECTION_OVERRIDES_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/overrides";
const RESOURCE_STRATEGY_SELECTION_APPROVAL_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/approval";

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

#[tokio::test]
async fn strategy_selection_mcp_surface_advertises_tools_resources_and_guidance_prompt() {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S01 strategy-selection handlers exist");

    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == TOOL_MATERIALIZE_STRATEGY_SELECTION),
        "S01 must advertise a strategy_selection.materialize tool so agents can materialize the canonical recommendation instead of relying on session memory"
    );
    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == TOOL_APPLY_STRATEGY_OVERRIDE),
        "S01 must advertise a strategy_selection.apply_override tool for typed owner overrides"
    );
    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == TOOL_APPROVE_STRATEGY_SELECTION),
        "S01 must advertise a strategy_selection.approve tool so approval provenance is a first-class mutation"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == RESOURCE_STRATEGY_SELECTION_SUMMARY_TEMPLATE),
        "S01 must advertise a read-only strategy-selection summary resource keyed by install/proposal/selection identity"
    );
    assert!(
        capabilities.resources.iter().any(|resource| {
            resource.uri_template == RESOURCE_STRATEGY_SELECTION_OVERRIDES_TEMPLATE
        }),
        "S01 must advertise a read-only strategy-selection overrides resource for canonical override history"
    );
    assert!(
        capabilities.resources.iter().any(|resource| {
            resource.uri_template == RESOURCE_STRATEGY_SELECTION_APPROVAL_TEMPLATE
        }),
        "S01 must advertise a read-only strategy-selection approval resource for canonical approval provenance"
    );
    assert!(
        capabilities
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_STRATEGY_SELECTION_GUIDANCE),
        "S01 must ship a reusable strategy-selection guidance prompt that points future agents at canonical resources"
    );
}

#[tokio::test]
async fn strategy_selection_mcp_flow_requires_typed_errors_and_reconnect_safe_resources() {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S01 strategy-selection handlers exist");
    let advertised_tools = capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let advertised_resources = capabilities
        .resources
        .iter()
        .map(|resource| resource.uri_template.as_str())
        .collect::<Vec<_>>();
    let advertised_prompts = capabilities
        .prompts
        .iter()
        .map(|prompt| prompt.name.as_str())
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
    let proposal_uri = proposal["proposal_uri"]
        .as_str()
        .expect("proposal uri string")
        .to_owned();
    let proposal_resource = read_resource_json(&first_client, proposal_uri.clone()).await;

    let mut gaps = Vec::new();
    for required_tool in [
        TOOL_MATERIALIZE_STRATEGY_SELECTION,
        TOOL_APPLY_STRATEGY_OVERRIDE,
        TOOL_APPROVE_STRATEGY_SELECTION,
    ] {
        if !advertised_tools.contains(&required_tool) {
            gaps.push(format!(
                "initialize() missing advertised tool {required_tool}"
            ));
        }
    }
    for required_resource in [
        RESOURCE_STRATEGY_SELECTION_SUMMARY_TEMPLATE,
        RESOURCE_STRATEGY_SELECTION_OVERRIDES_TEMPLATE,
        RESOURCE_STRATEGY_SELECTION_APPROVAL_TEMPLATE,
    ] {
        if !advertised_resources.contains(&required_resource) {
            gaps.push(format!(
                "initialize() missing advertised strategy-selection resource template {required_resource}"
            ));
        }
    }
    if !advertised_prompts.contains(&PROMPT_STRATEGY_SELECTION_GUIDANCE) {
        gaps.push(format!(
            "initialize() missing reusable prompt {PROMPT_STRATEGY_SELECTION_GUIDANCE}"
        ));
    }

    if gaps.is_empty() {
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
            .expect("selection_id string")
            .to_owned();

        let summary_uri = format!(
            "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/summary"
        );
        let overrides_uri = format!(
            "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/overrides"
        );
        let approval_uri = format!(
            "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/approval"
        );

        let summary_before_override = read_resource_json(&first_client, summary_uri.clone()).await;
        if summary_before_override["install_id"] != install_id {
            gaps.push("summary resource must retain install_id".to_owned());
        }
        if summary_before_override["proposal_id"] != proposal_id {
            gaps.push("summary resource must retain proposal_id".to_owned());
        }
        if summary_before_override["selection_id"] != selection_id {
            gaps.push("summary resource must retain selection_id".to_owned());
        }
        if summary_before_override["summary_uri"] != summary_uri {
            gaps.push(
                "summary resource must point summary_uri back to its own canonical resource"
                    .to_owned(),
            );
        }
        if summary_before_override["overrides_uri"] != overrides_uri {
            gaps.push(
                "summary resource must advertise the canonical overrides resource uri".to_owned(),
            );
        }
        if summary_before_override["approval_uri"] != approval_uri {
            gaps.push(
                "summary resource must advertise the canonical approval resource uri".to_owned(),
            );
        }
        if summary_before_override["status"] != "recommended" {
            gaps.push(
                "summary resource must show status=recommended immediately after materialize"
                    .to_owned(),
            );
        }
        if summary_before_override["created_at"] != summary_before_override["updated_at"] {
            gaps.push(
                "freshly materialized summary resources must start with created_at == updated_at"
                    .to_owned(),
            );
        }
        if summary_before_override["proposal_snapshot"]["proposal_readiness"]
            != proposal_resource["proposal"]["proposal_readiness"]
        {
            gaps.push("summary resource must preserve proposal_snapshot.proposal_readiness from the exact proposal packet".to_owned());
        }
        if summary_before_override["proposal_snapshot"]["capital_profile"]["completeness"]
            != proposal_resource["proposal"]["capital_profile"]["completeness"]
        {
            gaps.push("summary resource must preserve truthful capital completeness from the proposal packet".to_owned());
        }
        if !summary_before_override["readiness_sensitivity_summary"]
            ["readiness_sensitive_override_keys"]
            .as_array()
            .is_some_and(|keys| keys.iter().any(|key| key == "approve-max-spread-budget"))
        {
            gaps.push("summary resource must classify approve-max-spread-budget as readiness_sensitive".to_owned());
        }

        let invalid_override_error = call_tool_error(
            &first_client,
            TOOL_APPLY_STRATEGY_OVERRIDE,
            json_map([
                ("install_id", Value::String(install_id.clone())),
                ("proposal_id", Value::String(proposal_id.clone())),
                ("selection_id", Value::String(selection_id.clone())),
                (
                    "override",
                    json!({
                        "key": "unknown-selection-field",
                        "value": { "resolution": "approved" },
                        "rationale": "exercise typed mutation errors"
                    }),
                ),
            ]),
        )
        .await;
        if !invalid_override_error.contains("invalid_override_key") {
            gaps.push(format!(
                "invalid strategy override must surface typed invalid_override_key error, got {invalid_override_error}"
            ));
        }

        let override_result = call_tool_json(
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
                        "value": { "resolution": "approved" },
                        "rationale": "owner approved the max spread budget after reviewing the proposal"
                    }),
                ),
            ]),
        )
        .await;
        if override_result["selection_id"] != selection_id {
            gaps.push("override result must keep selection identity stable".to_owned());
        }
        if override_result["selection_revision"]
            .as_i64()
            .unwrap_or_default()
            < 2
        {
            gaps.push("applying an override must advance selection_revision".to_owned());
        }
        if override_result["status"] != "recommended" {
            gaps.push(
                "override mutation responses must keep status=recommended until explicit approval"
                    .to_owned(),
            );
        }

        let summary_after_override = read_resource_json(&first_client, summary_uri.clone()).await;
        if summary_after_override["selection_revision"] != override_result["selection_revision"] {
            gaps.push("summary resource must reread the current selection_revision after an override mutation".to_owned());
        }
        if summary_after_override["updated_at"]
            .as_str()
            .unwrap_or_default()
            < summary_after_override["created_at"]
                .as_str()
                .unwrap_or_default()
        {
            gaps.push("summary resource must not regress updated_at behind created_at after the first override".to_owned());
        }
        if summary_after_override["status"] != "recommended" {
            gaps.push(
                "summary resource must stay recommended after override until approval happens"
                    .to_owned(),
            );
        }

        let stale_approval_error = call_tool_error(
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
                        "note": "exercise stale approval detection"
                    }),
                ),
            ]),
        )
        .await;
        if !stale_approval_error.contains("stale_selection_revision") {
            gaps.push(format!(
                "stale approval must surface typed stale_selection_revision error, got {stale_approval_error}"
            ));
        }

        let approved = call_tool_json(
            &first_client,
            TOOL_APPROVE_STRATEGY_SELECTION,
            json_map([
                ("install_id", Value::String(install_id.clone())),
                ("proposal_id", Value::String(proposal_id.clone())),
                ("selection_id", Value::String(selection_id.clone())),
                (
                    "expected_selection_revision",
                    Value::from(override_result["selection_revision"].as_i64().unwrap_or(2)),
                ),
                (
                    "approval",
                    json!({
                        "approved_by": "owner",
                        "note": "approved after reviewing the canonical snapshot and override history"
                    }),
                ),
            ]),
        )
        .await;
        if approved["status"] != "approved" {
            gaps.push("approve tool must return status=approved".to_owned());
        }

        let summary_after_approval = read_resource_json(&first_client, summary_uri.clone()).await;
        if summary_after_approval["status"] != "approved" {
            gaps.push(
                "summary resource must reread approved status after the approval mutation"
                    .to_owned(),
            );
        }
        if summary_after_approval["updated_at"]
            .as_str()
            .unwrap_or_default()
            < summary_after_override["updated_at"]
                .as_str()
                .unwrap_or_default()
        {
            gaps.push(
                "summary resource must not regress updated_at when approval is persisted"
                    .to_owned(),
            );
        }
        if summary_after_approval["approval"]["approved_revision"]
            != override_result["selection_revision"]
        {
            gaps.push(
                "summary resource approval block must retain the exact approved revision"
                    .to_owned(),
            );
        }
        if summary_after_approval["approval"]["approved_by"] != "owner" {
            gaps.push(
                "summary resource approval block must retain approved_by provenance".to_owned(),
            );
        }
        if summary_after_approval["approval"]["approved_at"] != summary_after_approval["updated_at"]
        {
            gaps.push("summary resource approval block must stamp approved_at with the same timestamp as the approved summary".to_owned());
        }

        let overrides_before_restart =
            read_resource_json(&first_client, overrides_uri.clone()).await;
        let approval_before_restart = read_resource_json(&first_client, approval_uri.clone()).await;
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
        if !guidance_text.contains(&summary_uri)
            || !guidance_text.contains(&overrides_uri)
            || !guidance_text.contains(&approval_uri)
        {
            gaps.push("strategy-selection guidance prompt must reference summary, overrides, and approval resources".to_owned());
        }
        if !guidance_text.contains(TOOL_APPLY_STRATEGY_OVERRIDE)
            || !guidance_text.contains(TOOL_APPROVE_STRATEGY_SELECTION)
        {
            gaps.push("strategy-selection guidance prompt must keep override and approval tool names explicit".to_owned());
        }
        if !guidance_text.contains("Do not rely on prior mutation receipts or session memory") {
            gaps.push("strategy-selection guidance prompt must explicitly reject session-memory shortcuts".to_owned());
        }

        if overrides_before_restart["summary_uri"] != summary_uri
            || overrides_before_restart["overrides_uri"] != overrides_uri
            || overrides_before_restart["approval_uri"] != approval_uri
        {
            gaps.push(
                "overrides resource must cross-link the canonical summary/overrides/approval uris"
                    .to_owned(),
            );
        }
        if !overrides_before_restart["overrides"]
            .as_array()
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item["override_key"] == "approve-max-spread-budget"
                        && item["sensitivity_class"] == "readiness_sensitive"
                        && item["provenance"] == json!({ "source": "owner_override" })
                        && item["created_at"] == summary_after_override["updated_at"]
                })
            })
        {
            gaps.push(
                "overrides resource must expose typed readiness_sensitive override history with stable provenance and timestamp alignment"
                    .to_owned(),
            );
        }
        if approval_before_restart["status"] != "approved" {
            gaps.push("approval resource must expose approved status after approval".to_owned());
        }
        if approval_before_restart["summary_uri"] != summary_uri
            || approval_before_restart["overrides_uri"] != overrides_uri
            || approval_before_restart["approval_uri"] != approval_uri
        {
            gaps.push(
                "approval resource must cross-link the canonical summary/overrides/approval uris"
                    .to_owned(),
            );
        }
        if approval_before_restart["approved_by"] != "owner" {
            gaps.push("approval resource must preserve approved_by provenance".to_owned());
        }
        if approval_before_restart["approved_revision"] != override_result["selection_revision"] {
            gaps.push(
                "approval resource must preserve the exact approved selection revision".to_owned(),
            );
        }
        if approval_before_restart["approved_at"] != summary_after_approval["updated_at"] {
            gaps.push("approval resource must preserve the canonical approval timestamp from the approved summary".to_owned());
        }

        first_client
            .cancel()
            .await
            .expect("first live stdio server should shut down cleanly");

        let reconnected_client = spawn_live_client().await;
        let pre_reopen_summary_error =
            read_resource_error(&reconnected_client, summary_uri.clone()).await;
        if !(pre_reopen_summary_error.contains("install")
            || pre_reopen_summary_error.contains("locator"))
        {
            gaps.push(format!(
                "strategy-selection summary resource must reject reconnect reads before bootstrap reopen repopulates install locator, got {pre_reopen_summary_error}"
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
            gaps.push("bootstrap reopen must preserve install identity for reconnect-safe selection resources".to_owned());
        }

        let summary_after_restart =
            read_resource_json(&reconnected_client, summary_uri.clone()).await;
        let overrides_after_restart =
            read_resource_json(&reconnected_client, overrides_uri.clone()).await;
        let approval_after_restart =
            read_resource_json(&reconnected_client, approval_uri.clone()).await;

        if summary_after_restart["proposal_snapshot"]["proposal_readiness"]
            != summary_before_override["proposal_snapshot"]["proposal_readiness"]
        {
            gaps.push(
                "summary resource must reread canonical proposal_snapshot truth after reconnect"
                    .to_owned(),
            );
        }
        if overrides_after_restart != overrides_before_restart {
            gaps.push("overrides resource must reread the same canonical override history after reconnect".to_owned());
        }
        if approval_after_restart != approval_before_restart {
            gaps.push("approval resource must reread the same canonical approval provenance after reconnect".to_owned());
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
        "S01 MCP strategy-selection contract missing materialize/override/approve or reconnect-safe inspection: {}",
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
