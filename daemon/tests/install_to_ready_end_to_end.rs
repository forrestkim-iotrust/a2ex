mod support;

use std::path::{Path, PathBuf};

use a2ex_mcp::{PROMPT_ARGUMENT_SESSION_ID, PROMPT_PROPOSAL_PACKET, stable_session_id};
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
const TOOL_EVALUATE_ROUTE_READINESS: &str = "readiness.evaluate_route";

const PROMPT_CURRENT_STEP_GUIDANCE: &str = "onboarding.current_step_guidance";
const PROMPT_FAILURE_SUMMARY: &str = "onboarding.failure_summary";
const PROMPT_ROUTE_READINESS_GUIDANCE: &str = "readiness.route_guidance";
const PROMPT_ROUTE_BLOCKER_SUMMARY: &str = "readiness.route_blocker_summary";
const PROMPT_ARGUMENT_INSTALL_ID: &str = "install_id";

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

const DRIFT_ENTRY_SKILL_V2_MD: &str = r#"---
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

Track spread divergences after setup is complete.

# Owner Decisions

- Approve max spread budget.
"#;

const DRIFT_OWNER_SETUP_V2_MD: &str = r#"---
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
async fn install_ready_state_exposes_proposal_handoff_metadata_over_live_stdio() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let bootstrap = bootstrap_install(&client, &entry_url, workspace_root.path(), None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install_id string")
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
    assert!(
        ready["current_step_key"].is_null(),
        "ready onboarding should clear the current guided step once the install is proposal-ready"
    );

    let guided_state = read_resource_json(
        &client,
        format!("a2ex://onboarding/installs/{install_id}/guided_state"),
    )
    .await;
    assert_eq!(guided_state["aggregate_status"], "ready");
    assert_eq!(
        guided_state["attached_bundle_url"], entry_url,
        "S04 must expose the attached bundle URL from install-backed onboarding state so the handoff into skills.load_bundle is inspectable instead of implied"
    );
    assert_eq!(
        guided_state["proposal_handoff"]["tool_name"], TOOL_LOAD_BUNDLE,
        "S04 must publish an explicit proposal handoff contract once onboarding reaches ready"
    );
    assert_eq!(
        guided_state["proposal_handoff"]["entry_url"], entry_url,
        "handoff metadata must tell the agent which same attached bundle URL to load into skills.*"
    );
    assert_eq!(
        guided_state["proposal_handoff"]["next_tool_name"], TOOL_GENERATE_PROPOSAL_PACKET,
        "handoff metadata must name the proposal packet tool so the install-to-ready seam stays inspectable"
    );

    let current_step_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_CURRENT_STEP_GUIDANCE).with_arguments(json_map([(
                PROMPT_ARGUMENT_INSTALL_ID,
                Value::String(install_id),
            )])),
        )
        .await
        .expect("current step guidance prompt should render for ready installs");
    let current_step_prompt_text = prompt_text(&current_step_prompt);
    assert!(
        current_step_prompt_text.contains(TOOL_LOAD_BUNDLE)
            && current_step_prompt_text.contains(TOOL_GENERATE_PROPOSAL_PACKET),
        "S04 ready guidance must make the onboarding→skills handoff explicit in the shipped prompt surface"
    );

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly");
}

#[tokio::test]
async fn install_url_reaches_proposal_ready_over_one_live_stdio_server() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let tools = client
        .list_all_tools()
        .await
        .expect("live stdio server should advertise the assembled onboarding and skill tools");
    assert!(tools.iter().any(|tool| tool.name == TOOL_BOOTSTRAP_INSTALL));
    assert!(tools.iter().any(|tool| tool.name == TOOL_LOAD_BUNDLE));
    assert!(
        tools
            .iter()
            .any(|tool| tool.name == TOOL_GENERATE_PROPOSAL_PACKET)
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool.name == TOOL_EVALUATE_ROUTE_READINESS),
        "T04 must expose the route-readiness evaluation seam on the same live MCP server that handles onboarding and proposal handoff"
    );

    let resource_templates = client
        .list_resource_templates(None)
        .await
        .expect("live stdio server should list route-readiness resource templates");
    assert!(
        resource_templates
            .resource_templates
            .iter()
            .any(|template| {
                template.uri_template
                    == "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/summary"
            })
    );
    assert!(
        resource_templates
            .resource_templates
            .iter()
            .any(|template| {
                template.uri_template
                    == "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/blockers"
            })
    );

    let prompts = client
        .list_prompts(None)
        .await
        .expect("live stdio server should list route-readiness prompts");
    assert!(
        prompts
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_ROUTE_READINESS_GUIDANCE)
    );
    assert!(
        prompts
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_ROUTE_BLOCKER_SUMMARY)
    );

    let bootstrap = bootstrap_install(&client, &entry_url, workspace_root.path(), None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install_id string")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id string")
        .to_owned();
    assert_eq!(bootstrap["claim_disposition"], "claimed");
    assert_eq!(bootstrap["attached_bundle_url"], entry_url);

    let after_secret = apply_action(
        &client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    assert_eq!(
        after_secret["current_step_key"],
        "approve-max-spread-budget"
    );

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

    let guided_state = read_resource_json(
        &client,
        format!("a2ex://onboarding/installs/{install_id}/guided_state"),
    )
    .await;
    assert_eq!(guided_state["aggregate_status"], "ready");
    assert_eq!(guided_state["workspace_id"], workspace_id);

    let handoff_entry_url = guided_state["proposal_handoff"]["entry_url"]
        .as_str()
        .expect(
            "S04 ready installs must expose a proposal_handoff.entry_url so this acceptance path does not rely on out-of-band bootstrap knowledge",
        )
        .to_owned();
    assert_eq!(handoff_entry_url, entry_url);

    let load = call_tool_json(
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(handoff_entry_url))]),
    )
    .await;
    let session_id = load["session_id"]
        .as_str()
        .expect("session id string")
        .to_owned();
    let session_uri_root = load["session_uri_root"]
        .as_str()
        .expect("session uri root string")
        .to_owned();
    assert_eq!(
        session_id,
        stable_session_id(&entry_url),
        "S04 ready-path handoff must keep the skill session identity stable and derivable from the surfaced bundle URL"
    );
    assert!(
        session_uri_root.ends_with(&session_id),
        "session_uri_root should stay aligned with the stable session_id returned by skills.load_bundle"
    );

    let status = read_resource_json(&client, format!("{session_uri_root}/status")).await;
    assert_eq!(status["status"], "interpreted_ready");
    assert_eq!(status["proposal_readiness"], "ready");

    let proposal = call_tool_json(
        &client,
        TOOL_GENERATE_PROPOSAL_PACKET,
        json_map([("session_id", Value::String(session_id.clone()))]),
    )
    .await;
    assert_eq!(proposal["session_id"], session_id);
    assert_eq!(proposal["proposal_readiness"], "ready");

    let proposal_resource =
        read_resource_json(&client, format!("{session_uri_root}/proposal")).await;
    assert_eq!(proposal_resource["proposal"]["proposal_readiness"], "ready");
    assert_eq!(
        proposal_resource["handoff"]["install_id"], install_id,
        "S04 must preserve install-backed identity in the proposal resource so the final assembled handoff stays inspectable end-to-end"
    );
    assert_eq!(proposal_resource["handoff"]["workspace_id"], workspace_id);
    assert_eq!(
        proposal_resource["handoff"]["attached_bundle_url"], entry_url,
        "proposal resources must keep the original onboarding bundle URL inspectable after skills.load_bundle"
    );

    let proposal_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_PROPOSAL_PACKET).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(session_id.clone()),
            )])),
        )
        .await
        .expect("proposal packet prompt should render from the same live session root");
    let proposal_prompt_text = prompt_text(&proposal_prompt);
    assert!(
        proposal_prompt_text.contains(&format!("{session_uri_root}/proposal"))
            && proposal_prompt_text.contains(&session_id),
        "S04 ready-path acceptance must keep the proposal prompt readable from the same live MCP session"
    );

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly");
}

#[tokio::test]
async fn blocked_install_reopens_after_restart_with_readable_diagnostics() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");

    let first_client = spawn_live_client().await;
    let bootstrap =
        bootstrap_install(&first_client, &entry_url, workspace_root.path(), None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id string")
        .to_owned();

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(DRIFT_ENTRY_SKILL_V2_MD),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(DRIFT_OWNER_SETUP_V2_MD),
    );

    first_client
        .cancel()
        .await
        .expect("first live stdio server should shut down before restart coverage");

    let restarted_client = spawn_live_client().await;
    let reopened = bootstrap_install(
        &restarted_client,
        &entry_url,
        workspace_root.path(),
        Some(workspace_id.clone()),
        Some(install_id.clone()),
    )
    .await;
    assert_eq!(reopened["claim_disposition"], "reopened");
    assert_eq!(reopened["workspace_id"], workspace_id);
    assert_eq!(reopened["install_id"], install_id);
    assert_eq!(reopened["aggregate_status"], "drifted");

    let diagnostics = read_resource_json(
        &restarted_client,
        format!(
            "a2ex://onboarding/installs/{}/diagnostics",
            reopened["install_id"].as_str().expect("install id")
        ),
    )
    .await;
    assert_eq!(diagnostics["aggregate_status"], "drifted");
    assert_eq!(diagnostics["drift"]["classification"], "documents_changed");
    assert_eq!(
        diagnostics["attached_bundle_url"], entry_url,
        "S04 restart-safe reopen diagnostics must keep the attached bundle URL inspectable after server restart"
    );
    assert_eq!(
        diagnostics["bootstrap"]["used_remote_control_plane"], false,
        "S04 restart-safe reopen diagnostics must preserve the local-first bootstrap authority signal"
    );

    let failure_prompt = restarted_client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_FAILURE_SUMMARY).with_arguments(json_map([(
                PROMPT_ARGUMENT_INSTALL_ID,
                reopened["install_id"].clone(),
            )])),
        )
        .await
        .expect("failure summary prompt should render after restart-driven reopen");
    let failure_prompt_text = prompt_text(&failure_prompt);
    assert!(
        failure_prompt_text.contains("documents_changed")
            && failure_prompt_text.contains("KALSHI_API_KEY")
            && failure_prompt_text.contains("used_remote_control_plane=false"),
        "S04 blocked reopen prompts must preserve readable drift diagnostics and local-first authority context after restart"
    );

    restarted_client
        .cancel()
        .await
        .expect("restarted live stdio server should shut down cleanly");
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

async fn bootstrap_install(
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
    decode_structured_tool_result(
        client
            .call_tool(CallToolRequestParams::new(tool_name.to_owned()).with_arguments(arguments))
            .await
            .unwrap_or_else(|error| panic!("tool {tool_name} should succeed: {error}")),
    )
}

fn decode_structured_tool_result(result: rmcp::model::CallToolResult) -> Value {
    result
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
