mod support;

use std::path::PathBuf;

use a2ex_mcp::{
    GenerateProposalPacketResponse, LoadBundleResponse, PROMPT_ARGUMENT_SESSION_ID,
    PROMPT_OPERATOR_GUIDANCE, PROMPT_PROPOSAL_PACKET, PROMPT_STATUS_SUMMARY, SERVER_NAME,
    SessionProposalReadiness, SkillSessionResourceKind, TOOL_CLEAR_STOP,
    TOOL_GENERATE_PROPOSAL_PACKET, TOOL_LOAD_BUNDLE, TOOL_RELOAD_BUNDLE, TOOL_STOP_SESSION,
    session_uri_root, stable_session_id,
};
use a2ex_skill_bundle::{BundleDocumentLifecycleChangeKind, BundleLifecycleClassification};
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

const ENTRY_SKILL_MD_V1: &str = r#"---
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

Track spread divergences after all setup is complete.
"#;

const ENTRY_SKILL_MD_V2: &str = r#"---
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

Track spread divergences after all setup is complete.
"#;

const OWNER_SETUP_MD_V1: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
"#;

const OWNER_SETUP_MD_V2: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.11
---
"#;

#[tokio::test]
async fn live_stdio_session_covers_load_reload_lifecycle_stop_and_clear_from_one_bundle_url() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD_V1),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD_V1),
        ),
    ])
    .await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");

    let client = spawn_live_client().await;

    let server_info = client
        .peer_info()
        .expect("initialized server info should be available for the live S07 proof");
    assert_eq!(server_info.server_info.name, SERVER_NAME);

    let tools = client
        .list_all_tools()
        .await
        .expect("live stdio server should list the shipped skill tools");
    let tool_names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(
        [
            TOOL_LOAD_BUNDLE,
            TOOL_RELOAD_BUNDLE,
            TOOL_GENERATE_PROPOSAL_PACKET,
            TOOL_STOP_SESSION,
            TOOL_CLEAR_STOP,
        ]
        .into_iter()
        .all(|required_tool| tool_names.contains(&required_tool)),
        "S07 requires the shipped skill tools to remain available even after the shared live MCP server also exposes onboarding tools"
    );

    let prompts = client
        .list_prompts(None)
        .await
        .expect("live stdio server should list the shipped prompts");
    assert!(
        prompts
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_STATUS_SUMMARY)
    );
    assert!(
        prompts
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_PROPOSAL_PACKET)
    );
    assert!(
        prompts
            .prompts
            .iter()
            .any(|prompt| prompt.name == PROMPT_OPERATOR_GUIDANCE)
    );
    for prompt_name in [
        PROMPT_STATUS_SUMMARY,
        PROMPT_PROPOSAL_PACKET,
        PROMPT_OPERATOR_GUIDANCE,
    ] {
        let prompt = prompts
            .prompts
            .iter()
            .find(|prompt| prompt.name == prompt_name)
            .expect("shipped skill prompt should be listed");
        assert_eq!(
            prompt
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.first())
                .map(|argument| argument.name.as_str()),
            Some(PROMPT_ARGUMENT_SESSION_ID),
            "skill prompts must still take session_id even when onboarding prompts share the same server"
        );
    }

    let load: LoadBundleResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_LOAD_BUNDLE)
                    .with_arguments(json_map([("entry_url", Value::String(entry_url.clone()))])),
            )
            .await
            .expect("live stdio server should load the initial ready bundle"),
    );
    let session_root = load.session_uri_root.clone();
    assert_eq!(load.entry_url, entry_url);
    assert_eq!(
        load.resource_uris,
        SkillSessionResourceKind::all()
            .into_iter()
            .map(|resource| resource.uri_for_session(&load.session_id))
            .collect::<Vec<_>>(),
        "load metadata must advertise the stable session-backed inspection surface"
    );

    let first_status = read_resource_json(&client, format!("{session_root}/status")).await;
    assert_eq!(first_status["session_id"], load.session_id);
    assert_eq!(first_status["entry_url"], entry_url);
    assert_eq!(first_status["status"], "interpreted_ready");
    assert_eq!(first_status["revision"], 1);
    assert_eq!(first_status["proposal_readiness"], "ready");

    let first_generation: GenerateProposalPacketResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_GENERATE_PROPOSAL_PACKET).with_arguments(json_map(
                    [("session_id", Value::String(load.session_id.clone()))],
                )),
            )
            .await
            .expect("ready sessions should generate a proposal packet over the live stdio server"),
    );
    assert_eq!(first_generation.session_id, load.session_id);
    assert_eq!(first_generation.session_uri_root, session_root);
    assert_eq!(
        first_generation.proposal_uri,
        format!("{session_root}/proposal")
    );
    assert_eq!(first_generation.revision, 1);
    assert_eq!(first_generation.proposal_revision, 1);
    assert_eq!(
        first_generation.proposal_readiness,
        SessionProposalReadiness::Ready
    );

    let first_proposal = read_resource_json(&client, format!("{session_root}/proposal")).await;
    assert_eq!(first_proposal["proposal_revision"], 1);
    assert_eq!(first_proposal["proposal"]["proposal_readiness"], "ready");

    let status_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_STATUS_SUMMARY).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(load.session_id.clone()),
            )])),
        )
        .await
        .expect("status prompt should render from the same live session root");
    let status_prompt_text = prompt_text(&status_prompt);
    assert!(status_prompt_text.contains(&load.session_id));
    assert!(status_prompt_text.contains("Blockers: 0"));

    let proposal_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_PROPOSAL_PACKET).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(load.session_id.clone()),
            )])),
        )
        .await
        .expect("proposal prompt should render against the live stdio session");
    let proposal_prompt_text = prompt_text(&proposal_prompt);
    assert!(proposal_prompt_text.contains(&format!("{session_root}/proposal")));

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_MD_V2),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_MD_V2),
    );

    let reload: LoadBundleResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_RELOAD_BUNDLE).with_arguments(json_map([
                    ("session_id", Value::String(load.session_id.clone())),
                    ("entry_url", Value::String(entry_url.clone())),
                ])),
            )
            .await
            .expect("same-url reload should stay on the original live MCP session"),
    );
    assert_eq!(reload.session_id, load.session_id);
    assert_eq!(reload.session_uri_root, session_root);
    assert_eq!(reload.entry_url, entry_url);

    let lifecycle = read_resource_json(&client, format!("{session_root}/lifecycle")).await;
    assert_eq!(lifecycle["session_id"], load.session_id);
    assert_eq!(lifecycle["revision"], 2);
    assert_eq!(
        lifecycle["lifecycle"]["classification"],
        serde_json::to_value(BundleLifecycleClassification::DocumentsChanged)
            .expect("documents_changed serializes")
    );
    assert!(
        lifecycle["lifecycle"]["changed_documents"]
            .as_array()
            .expect("changed_documents should be present after same-url mutation")
            .iter()
            .any(|change| {
                change["document_id"] == "owner-setup"
                    && change["kind"]
                        == serde_json::to_value(BundleDocumentLifecycleChangeKind::RevisionChanged)
                            .expect("revision_changed serializes")
                    && change["previous_revision"] == "2026.03.10"
                    && change["current_revision"] == "2026.03.11"
            }),
        "same-url reload must expose lifecycle document drift from the stable session root"
    );

    let operator_state =
        read_resource_json(&client, format!("{session_root}/operator_state")).await;
    assert_eq!(operator_state["session_id"], load.session_id);
    assert_eq!(operator_state["revision"], 2);
    assert_eq!(operator_state["stop_state"], "active");
    assert_eq!(operator_state["proposal_readiness"], "ready");
    assert_eq!(
        operator_state["lifecycle_classification"],
        serde_json::to_value(BundleLifecycleClassification::DocumentsChanged)
            .expect("documents_changed serializes")
    );
    assert_eq!(
        operator_state["last_command_outcome"]["command"],
        TOOL_RELOAD_BUNDLE
    );

    let stop = call_tool_json(
        &client,
        TOOL_STOP_SESSION,
        json_map([("session_id", Value::String(load.session_id.clone()))]),
    )
    .await;
    assert_eq!(stop["session_id"], load.session_id);
    assert_eq!(stop["session_uri_root"], session_root);
    assert_eq!(stop["stop_state"], "stopped");

    let rejected_generation = client
        .call_tool(
            CallToolRequestParams::new(TOOL_GENERATE_PROPOSAL_PACKET).with_arguments(json_map([(
                "session_id",
                Value::String(load.session_id.clone()),
            )])),
        )
        .await;
    assert!(
        rejected_generation.is_err(),
        "stopped sessions must reject proposal generation in the live end-to-end flow"
    );

    let failures_after_rejection =
        read_resource_json(&client, format!("{session_root}/failures")).await;
    assert_eq!(failures_after_rejection["session_id"], load.session_id);
    assert_eq!(failures_after_rejection["stop_state"], "stopped");
    assert_eq!(
        failures_after_rejection["last_rejected_command"]["command"],
        TOOL_GENERATE_PROPOSAL_PACKET
    );
    assert_eq!(
        failures_after_rejection["last_rejected_command"]["rejection_code"],
        "session_stopped"
    );
    assert_eq!(
        failures_after_rejection["last_command_outcome"]["rejection_code"],
        "session_stopped"
    );

    let clear = call_tool_json(
        &client,
        TOOL_CLEAR_STOP,
        json_map([("session_id", Value::String(load.session_id.clone()))]),
    )
    .await;
    assert_eq!(clear["session_id"], load.session_id);
    assert_eq!(clear["session_uri_root"], session_root);
    assert_eq!(clear["stop_state"], "active");

    let cleared_operator_state =
        read_resource_json(&client, format!("{session_root}/operator_state")).await;
    assert_eq!(cleared_operator_state["session_id"], load.session_id);
    assert_eq!(cleared_operator_state["revision"], 5);
    assert_eq!(cleared_operator_state["stop_state"], "active");
    assert_eq!(cleared_operator_state["clearable"], false);
    assert_eq!(cleared_operator_state["stoppable"], true);
    assert_eq!(
        cleared_operator_state["last_command_outcome"]["command"],
        TOOL_CLEAR_STOP
    );

    let failures_after_clear =
        read_resource_json(&client, format!("{session_root}/failures")).await;
    assert_eq!(failures_after_clear["session_id"], load.session_id);
    assert_eq!(failures_after_clear["revision"], 5);
    assert_eq!(failures_after_clear["stop_state"], "active");
    assert_eq!(
        failures_after_clear["last_rejected_command"]["command"], TOOL_GENERATE_PROPOSAL_PACKET,
        "clear_stop must not erase the rejected-command diagnostic that explained why proposal generation failed while stopped"
    );
    assert_eq!(
        failures_after_clear["last_rejected_command"]["rejection_code"],
        "session_stopped"
    );

    let status_after_clear = read_resource_json(&client, format!("{session_root}/status")).await;
    assert_eq!(status_after_clear["session_id"], load.session_id);
    assert_eq!(status_after_clear["entry_url"], entry_url);
    assert_eq!(status_after_clear["revision"], 5);
    assert_eq!(status_after_clear["status"], "interpreted_ready");
    assert_eq!(status_after_clear["proposal_readiness"], "ready");
    assert_eq!(
        status_after_clear["proposal_uri"],
        format!("{session_root}/proposal")
    );

    let proposal_after_clear =
        read_resource_json(&client, format!("{session_root}/proposal")).await;
    assert_eq!(proposal_after_clear["session_id"], load.session_id);
    assert_eq!(
        proposal_after_clear["proposal_uri"],
        format!("{session_root}/proposal")
    );
    assert_eq!(proposal_after_clear["proposal_revision"], 5);
    assert_eq!(
        proposal_after_clear["proposal"]["proposal_readiness"],
        "ready"
    );

    let operator_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_OPERATOR_GUIDANCE).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(load.session_id.clone()),
            )])),
        )
        .await
        .expect("operator guidance prompt should stay readable after stop and clear_stop");
    let operator_prompt_text = prompt_text(&operator_prompt);
    assert!(operator_prompt_text.contains("operator_state"));
    assert!(operator_prompt_text.contains("failures"));
    assert!(operator_prompt_text.contains(&load.session_id));

    let mismatched_entry_url = format!("{entry_url}?identity-mismatch=1");
    let mismatched_reload = client
        .call_tool(
            CallToolRequestParams::new(TOOL_RELOAD_BUNDLE).with_arguments(json_map([
                ("session_id", Value::String(load.session_id.clone())),
                ("entry_url", Value::String(mismatched_entry_url.clone())),
            ])),
        )
        .await;
    assert!(
        mismatched_reload.is_err(),
        "reload with different URL text must reject instead of silently forking the session identity"
    );

    let mismatched_session_root = session_uri_root(&stable_session_id(&mismatched_entry_url));
    let mismatched_status = client
        .read_resource(ReadResourceRequestParams::new(format!(
            "{mismatched_session_root}/status"
        )))
        .await;
    assert!(
        mismatched_status.is_err(),
        "identity-mismatched reload must reject before mutating the registry or creating a stray session root"
    );

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly after the S07 proof");
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
