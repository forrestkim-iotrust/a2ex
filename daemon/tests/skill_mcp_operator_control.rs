mod support;

use std::path::PathBuf;

use a2ex_mcp::{
    PROMPT_ARGUMENT_SESSION_ID, PROMPT_OPERATOR_GUIDANCE, PROMPT_OWNER_GUIDANCE,
    PROMPT_PROPOSAL_PACKET, PROMPT_STATUS_SUMMARY, SERVER_NAME, TOOL_CLEAR_STOP,
    TOOL_GENERATE_PROPOSAL_PACKET, TOOL_LOAD_BUNDLE, TOOL_STOP_SESSION,
};
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams, ResourceContents,
    },
    transport::TokioChildProcess,
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use support::skill_bundle_harness::{BundleFixture, spawn_skill_bundle};

const BLOCKED_ENTRY_SKILL_MD: &str = r#"---
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

Track spread divergences and wait for explicit owner approval before acting.

# Owner Decisions

- Approve max spread budget.
"#;

#[tokio::test]
async fn blocked_session_operator_resources_define_the_s06_contract() {
    assert_eq!(TOOL_STOP_SESSION, "skills.stop_session");
    assert_eq!(TOOL_CLEAR_STOP, "skills.clear_stop");
    assert_eq!(PROMPT_OPERATOR_GUIDANCE, "skills.operator_guidance");

    let harness = spawn_skill_bundle([(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(BLOCKED_ENTRY_SKILL_MD),
    )])
    .await;

    let client = spawn_live_client().await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");

    let load = call_tool_json(
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.clone()))]),
    )
    .await;

    let session_id = load["session_id"]
        .as_str()
        .expect("load_bundle should return a session id")
        .to_owned();
    let session_uri_root = format!("a2ex://skills/sessions/{session_id}");

    assert_eq!(load["entry_url"], entry_url);
    assert_eq!(load["status"], "blocked");
    assert_eq!(load["blocker_count"], 1);
    assert_eq!(load["ambiguity_count"], 0);
    assert_eq!(
        load["resource_uris"].as_array().cloned(),
        Some(vec![
            Value::String(format!("{session_uri_root}/status")),
            Value::String(format!("{session_uri_root}/bundle")),
            Value::String(format!("{session_uri_root}/interpretation")),
            Value::String(format!("{session_uri_root}/blockers")),
            Value::String(format!("{session_uri_root}/ambiguities")),
            Value::String(format!("{session_uri_root}/provenance")),
            Value::String(format!("{session_uri_root}/lifecycle")),
            Value::String(format!("{session_uri_root}/proposal")),
            Value::String(format!("{session_uri_root}/operator_state")),
            Value::String(format!("{session_uri_root}/failures")),
        ]),
        "S06 load responses must advertise the operator_state and failures resources alongside the existing MCP session surface"
    );
    assert_eq!(
        load["prompt_names"].as_array().cloned(),
        Some(vec![
            Value::String(PROMPT_STATUS_SUMMARY.to_owned()),
            Value::String(PROMPT_OWNER_GUIDANCE.to_owned()),
            Value::String(PROMPT_PROPOSAL_PACKET.to_owned()),
            Value::String(PROMPT_OPERATOR_GUIDANCE.to_owned()),
        ]),
        "S06 load responses must advertise a dedicated operator guidance prompt instead of hiding stop/failure guidance in transient tool text"
    );

    let status = read_resource_json(&client, format!("{session_uri_root}/status")).await;
    assert_eq!(status["status"], "blocked");
    assert_eq!(status["blocker_count"], 1);

    let operator_state =
        read_resource_json(&client, format!("{session_uri_root}/operator_state")).await;
    assert_eq!(operator_state["session_id"], session_id);
    assert_eq!(operator_state["entry_url"], entry_url);
    assert_eq!(operator_state["stop_state"], "active");
    assert_eq!(operator_state["stoppable"], true);
    assert_eq!(operator_state["blocker_count"], 1);
    assert_eq!(operator_state["ambiguity_count"], 0);
    assert_eq!(operator_state["required_owner_action_count"], 1);
    assert_eq!(operator_state["proposal_readiness"], "blocked");
    assert_eq!(
        operator_state["next_operator_step"]["kind"], "supply_required_document",
        "S06 operator_state must explain what the operator needs to do next instead of only repeating the interpretation status"
    );
    assert_eq!(
        operator_state["last_command_outcome"]["command"], TOOL_LOAD_BUNDLE,
        "operator_state should keep an inspectable command outcome trail"
    );

    let failures = read_resource_json(&client, format!("{session_uri_root}/failures")).await;
    assert_eq!(failures["session_id"], session_id);
    assert_eq!(failures["stop_state"], "active");
    assert_eq!(failures["blocker_count"], 1);
    assert_eq!(failures["ambiguity_count"], 0);
    assert_eq!(failures["required_owner_action_count"], 1);
    assert_eq!(
        failures["current_failures"][0]["diagnostic_code"],
        "missing_required_document"
    );
    assert_eq!(
        failures["current_failures"][0]["owner_action_required"], true,
        "blocked diagnostics must preserve owner-action visibility in a readable failure surface"
    );

    let operator_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_OPERATOR_GUIDANCE).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(session_id.clone()),
            )])),
        )
        .await
        .expect("operator guidance prompt should be discoverable once S06 lands");
    let operator_prompt_text = prompt_text(&operator_prompt);
    assert!(operator_prompt_text.contains("missing_required_document"));
    assert!(operator_prompt_text.contains("operator_state"));
    assert!(operator_prompt_text.contains("failures"));

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly");
}

#[tokio::test]
async fn stop_and_clear_stop_must_preserve_inspectable_truth_for_stopped_sessions() {
    let harness = spawn_skill_bundle([(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(BLOCKED_ENTRY_SKILL_MD),
    )])
    .await;

    let client = spawn_live_client().await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");

    let load = call_tool_json(
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.clone()))]),
    )
    .await;
    let session_id = load["session_id"]
        .as_str()
        .expect("load_bundle should return a session id")
        .to_owned();
    let session_uri_root = format!("a2ex://skills/sessions/{session_id}");

    let stop = call_tool_json(
        &client,
        TOOL_STOP_SESSION,
        json_map([("session_id", Value::String(session_id.clone()))]),
    )
    .await;
    assert_eq!(stop["session_id"], session_id);
    assert_eq!(stop["stop_state"], "stopped");
    assert_eq!(stop["stopped_at_ms"].as_u64().is_some(), true);
    assert_eq!(
        stop["operator_state_uri"],
        format!("{session_uri_root}/operator_state")
    );
    assert_eq!(stop["failures_uri"], format!("{session_uri_root}/failures"));

    let stopped_operator_state =
        read_resource_json(&client, format!("{session_uri_root}/operator_state")).await;
    assert_eq!(stopped_operator_state["stop_state"], "stopped");
    assert_eq!(stopped_operator_state["stoppable"], false);
    assert_eq!(stopped_operator_state["clearable"], true);
    assert_eq!(
        stopped_operator_state["last_command_outcome"]["command"],
        TOOL_STOP_SESSION
    );

    let stopped_generation = client
        .call_tool(
            CallToolRequestParams::new(TOOL_GENERATE_PROPOSAL_PACKET).with_arguments(json_map([(
                "session_id",
                Value::String(session_id.clone()),
            )])),
        )
        .await;
    assert!(
        stopped_generation.is_err(),
        "stopped sessions should reject proposal generation until clear_stop is invoked"
    );

    let failures_after_rejection =
        read_resource_json(&client, format!("{session_uri_root}/failures")).await;
    assert_eq!(failures_after_rejection["stop_state"], "stopped");
    assert_eq!(
        failures_after_rejection["last_rejected_command"]["command"], TOOL_GENERATE_PROPOSAL_PACKET,
        "S06 failure resources must preserve the rejected command as structured truth after a stopped action"
    );
    assert_eq!(
        failures_after_rejection["last_rejected_command"]["rejection_code"],
        "session_stopped"
    );

    let clear = call_tool_json(
        &client,
        TOOL_CLEAR_STOP,
        json_map([("session_id", Value::String(session_id.clone()))]),
    )
    .await;
    assert_eq!(clear["session_id"], session_id);
    assert_eq!(clear["stop_state"], "active");
    assert_eq!(
        clear["operator_state_uri"],
        format!("{session_uri_root}/operator_state")
    );
    assert_eq!(
        clear["failures_uri"],
        format!("{session_uri_root}/failures")
    );

    let cleared_operator_state =
        read_resource_json(&client, format!("{session_uri_root}/operator_state")).await;
    assert_eq!(cleared_operator_state["stop_state"], "active");
    assert_eq!(cleared_operator_state["stoppable"], true);
    assert_eq!(cleared_operator_state["clearable"], false);
    assert_eq!(
        cleared_operator_state["last_command_outcome"]["command"],
        TOOL_CLEAR_STOP
    );

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly");
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
        .expect("initialized server info should be available to the client");
    assert_eq!(server_info.server_info.name, SERVER_NAME);

    client
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

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn decode_structured_tool_result<T>(result: rmcp::model::CallToolResult) -> T
where
    T: DeserializeOwned,
{
    serde_json::from_value(
        result
            .structured_content
            .expect("tool result should include structured content"),
    )
    .expect("structured tool content should deserialize into the expected response")
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
