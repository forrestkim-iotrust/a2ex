mod support;

use std::path::PathBuf;

use a2ex_mcp::A2exSkillMcpServer;
use reqwest::Client;
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams, ResourceContents,
    },
    transport::TokioChildProcess,
};
use serde_json::{Map, Value};
use support::skill_bundle_harness::{BundleFixture, spawn_skill_bundle};
use tempfile::tempdir;

const ONBOARDING_TOOL_BOOTSTRAP: &str = "onboarding.bootstrap_install";
const ONBOARDING_TOOL_APPLY_ACTION: &str = "onboarding.apply_action";
const ONBOARDING_PROMPT_CURRENT_STEP_GUIDANCE: &str = "onboarding.current_step_guidance";
const ONBOARDING_PROMPT_FAILURE_SUMMARY: &str = "onboarding.failure_summary";
const ONBOARDING_PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";
const GUIDED_STATE_RESOURCE_TEMPLATE: &str = "a2ex://onboarding/installs/{install_id}/guided_state";
const CHECKLIST_RESOURCE_TEMPLATE: &str = "a2ex://onboarding/installs/{install_id}/checklist";
const DIAGNOSTICS_RESOURCE_TEMPLATE: &str = "a2ex://onboarding/installs/{install_id}/diagnostics";

const GUIDED_ENTRY_SKILL_MD: &str = r#"---
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

Track spread divergences after local setup is complete.

# Owner Decisions

- Approve max spread budget.
"#;

const OWNER_SETUP_WITH_SECRET_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

const GUIDED_ENTRY_SKILL_V2_MD: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.13
compatible_daemon: ">=0.1.0"
name: Prediction Spread Arb
summary: Capture spread dislocations between prediction venues.
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
    revision: 2026.03.11
---
# Overview

Track spread divergences after local setup is complete.

# Owner Decisions

- Approve max spread budget.
"#;

const OWNER_SETUP_WITH_TWO_SECRETS_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.11
---
# Required Secrets

- POLYMARKET_API_KEY
- KALSHI_API_KEY
"#;

#[tokio::test]
async fn onboarding_mcp_contract_advertises_guided_tools_resources_and_prompts() {
    let server = A2exSkillMcpServer::new(Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before guided onboarding handlers exist");

    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == ONBOARDING_TOOL_BOOTSTRAP),
        "S03 must advertise a canonical onboarding bootstrap tool instead of forcing agents to bootstrap installs out-of-band"
    );
    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name == ONBOARDING_TOOL_APPLY_ACTION),
        "S03 must advertise an explicit onboarding action tool instead of requiring direct SQLite edits"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == GUIDED_STATE_RESOURCE_TEMPLATE),
        "S03 must advertise a guided onboarding state resource with stable install-scoped URIs"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == CHECKLIST_RESOURCE_TEMPLATE),
        "S03 must advertise a checklist resource separate from tool output"
    );
    assert!(
        capabilities
            .resources
            .iter()
            .any(|resource| resource.uri_template == DIAGNOSTICS_RESOURCE_TEMPLATE),
        "S03 must advertise a diagnostics resource for blocked or drifted installs"
    );
    assert!(
        capabilities
            .prompts
            .iter()
            .any(|prompt| prompt.name == ONBOARDING_PROMPT_CURRENT_STEP_GUIDANCE),
        "S03 must expose a current-step guidance prompt backed by guided onboarding resources"
    );
    assert!(
        capabilities
            .prompts
            .iter()
            .any(|prompt| prompt.name == ONBOARDING_PROMPT_FAILURE_SUMMARY),
        "S03 must expose a readable failure-summary prompt for blocked or drifted installs"
    );
}

#[tokio::test]
async fn onboarding_mcp_bootstrap_reopen_resources_actions_and_diagnostics_run_through_live_stdio()
{
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(GUIDED_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_WITH_SECRET_MD),
        ),
    ])
    .await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let workspace_root = tempdir().expect("workspace tempdir");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let daemon_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = daemon_root.join("Cargo.toml");

    let mut command = tokio::process::Command::new(cargo);
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&workspace_manifest)
        .arg("-p")
        .arg("a2ex-mcp")
        .arg("--bin")
        .arg("a2ex-mcp")
        .current_dir(&daemon_root);

    let transport = TokioChildProcess::new(command).expect("live stdio server should spawn");
    let client = ()
        .serve(transport)
        .await
        .expect("rmcp client should initialize against the live onboarding MCP server");

    let bootstrap: Value = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(ONBOARDING_TOOL_BOOTSTRAP).with_arguments(json_map([
                    ("install_url", Value::String(entry_url.clone())),
                    (
                        "workspace_root",
                        Value::String(workspace_root.path().display().to_string()),
                    ),
                ])),
            )
            .await
            .expect("S03 MCP surface must bootstrap a canonical onboarding install over stdio"),
    );

    assert_eq!(bootstrap["claim_disposition"], "claimed");
    assert_eq!(bootstrap["attached_bundle_url"], entry_url);
    assert_eq!(bootstrap["current_step_key"], "POLYMARKET_API_KEY");
    assert_eq!(bootstrap["recommended_action"]["kind"], "complete_step");
    assert_eq!(
        bootstrap["guided_state_uri"],
        format!(
            "a2ex://onboarding/installs/{}/guided_state",
            bootstrap["install_id"].as_str().expect("install id string")
        )
    );
    assert_eq!(
        bootstrap["checklist_uri"],
        format!(
            "a2ex://onboarding/installs/{}/checklist",
            bootstrap["install_id"].as_str().expect("install id string")
        )
    );
    assert_eq!(
        bootstrap["diagnostics_uri"],
        format!(
            "a2ex://onboarding/installs/{}/diagnostics",
            bootstrap["install_id"].as_str().expect("install id string")
        )
    );

    let guided_state = read_resource_json(
        &client,
        bootstrap["guided_state_uri"]
            .as_str()
            .expect("guided state uri")
            .to_owned(),
    )
    .await;
    assert_eq!(guided_state["current_step_key"], "POLYMARKET_API_KEY");
    assert_eq!(
        guided_state["ordered_steps"][0]["step_key"],
        "POLYMARKET_API_KEY"
    );
    assert_eq!(
        guided_state["ordered_steps"][1]["step_key"],
        "approve-max-spread-budget"
    );
    assert_eq!(guided_state["recommended_action"]["kind"], "complete_step");

    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id")
        .to_owned();

    let action_result: Value = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(ONBOARDING_TOOL_APPLY_ACTION).with_arguments(json_map([
                    ("install_id", Value::String(install_id.clone())),
                    (
                        "action",
                        serde_json::json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
                    ),
                ])),
            )
            .await
            .expect("S03 MCP surface must apply explicit onboarding actions over stdio"),
    );
    assert_eq!(action_result["install_id"], install_id);
    assert_eq!(
        action_result["current_step_key"],
        "approve-max-spread-budget"
    );
    assert_eq!(
        action_result["recommended_action"]["kind"],
        "resolve_owner_decision"
    );

    let reopened: Value = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(ONBOARDING_TOOL_BOOTSTRAP).with_arguments(json_map([
                    ("install_url", Value::String(entry_url.clone())),
                    (
                        "workspace_root",
                        Value::String(workspace_root.path().display().to_string()),
                    ),
                    ("expected_workspace_id", Value::String(workspace_id.clone())),
                    ("expected_install_id", Value::String(install_id.clone())),
                ])),
            )
            .await
            .expect("same install should reopen through the canonical onboarding bootstrap tool"),
    );
    assert_eq!(reopened["claim_disposition"], "reopened");
    assert_eq!(reopened["workspace_id"], workspace_id);
    assert_eq!(reopened["install_id"], install_id);
    assert_eq!(reopened["current_step_key"], "approve-max-spread-budget");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(GUIDED_ENTRY_SKILL_V2_MD),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_WITH_TWO_SECRETS_MD),
    );

    let drifted: Value = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(ONBOARDING_TOOL_BOOTSTRAP).with_arguments(json_map([
                    ("install_url", Value::String(entry_url.clone())),
                    (
                        "workspace_root",
                        Value::String(workspace_root.path().display().to_string()),
                    ),
                    ("expected_workspace_id", Value::String(workspace_id)),
                    ("expected_install_id", Value::String(install_id.clone())),
                ])),
            )
            .await
            .expect("reopening the same install after bundle drift should still return canonical onboarding metadata"),
    );
    assert_eq!(drifted["claim_disposition"], "reopened");
    assert_eq!(drifted["current_step_key"], "bundle_drift");
    assert_eq!(
        drifted["recommended_action"]["kind"],
        "acknowledge_bundle_drift"
    );

    let diagnostics = read_resource_json(
        &client,
        drifted["diagnostics_uri"]
            .as_str()
            .expect("diagnostics uri")
            .to_owned(),
    )
    .await;
    assert_eq!(diagnostics["aggregate_status"], "drifted");
    assert_eq!(diagnostics["attached_bundle_url"], entry_url);
    assert_eq!(
        diagnostics["bootstrap"]["used_remote_control_plane"], false,
        "install-backed diagnostics must preserve the local-first bootstrap authority signal"
    );
    assert_eq!(diagnostics["drift"]["classification"], "documents_changed");
    assert!(
        diagnostics["drift"]["changed_documents"]
            .as_array()
            .is_some_and(|changes| changes
                .iter()
                .any(|change| change["document_id"] == "owner-setup")),
        "drift diagnostics should name the changed document"
    );

    let failure_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(ONBOARDING_PROMPT_FAILURE_SUMMARY).with_arguments(
                json_map([(
                    ONBOARDING_PROMPT_ARGUMENT_INSTALL_ID,
                    Value::String(install_id.clone()),
                )]),
            ),
        )
        .await
        .expect("S03 MCP surface must render a failure-summary prompt for drifted installs");
    let failure_prompt_text = prompt_text(&failure_prompt);
    assert!(failure_prompt_text.contains("documents_changed"));
    assert!(failure_prompt_text.contains("bundle_drift"));
    assert!(failure_prompt_text.contains("used_remote_control_plane=false"));
    assert!(failure_prompt_text.contains("KALSHI_API_KEY"));

    client
        .call_tool(
            CallToolRequestParams::new(ONBOARDING_TOOL_APPLY_ACTION).with_arguments(json_map([
                ("install_id", Value::String(install_id.clone())),
                (
                    "action",
                    serde_json::json!({ "kind": "resolve_owner_decision", "step_key": "approve-max-spread-budget", "resolution": "approved" }),
                ),
            ])),
        )
        .await
        .expect_err("S03 MCP surface must reject out-of-order actions while drift review is pending");

    let diagnostics_after_rejection = read_resource_json(
        &client,
        format!("a2ex://onboarding/installs/{install_id}/diagnostics"),
    )
    .await;
    assert_eq!(
        diagnostics_after_rejection["last_rejection"]["code"],
        "bundle_drift_review_required"
    );

    let current_step_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(ONBOARDING_PROMPT_CURRENT_STEP_GUIDANCE).with_arguments(
                json_map([(
                    ONBOARDING_PROMPT_ARGUMENT_INSTALL_ID,
                    Value::String(install_id),
                )]),
            ),
        )
        .await
        .expect("S03 MCP surface must render current-step guidance for the canonical install");
    let current_step_prompt_text = prompt_text(&current_step_prompt);
    assert!(current_step_prompt_text.contains("bundle_drift"));
    assert!(current_step_prompt_text.contains("acknowledge_bundle_drift"));

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly");
}

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn decode_structured_tool_result(result: rmcp::model::CallToolResult) -> Value {
    result
        .structured_content
        .expect("tool result should include structured content")
}

async fn read_resource_json(client: &rmcp::service::Peer<rmcp::RoleClient>, uri: String) -> Value {
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
