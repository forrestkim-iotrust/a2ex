mod support;

use std::path::PathBuf;

use a2ex_mcp::{
    A2exSkillMcpServer, GenerateProposalPacketRequest, GenerateProposalPacketResponse,
    LoadBundleRequest, LoadBundleResponse, PROMPT_ARGUMENT_INSTALL_ID, PROMPT_ARGUMENT_PROPOSAL_ID,
    PROMPT_ARGUMENT_ROUTE_ID, PROMPT_ARGUMENT_SELECTION_ID, PROMPT_ARGUMENT_SESSION_ID,
    PROMPT_CURRENT_STEP_GUIDANCE, PROMPT_FAILURE_SUMMARY, PROMPT_OPERATOR_GUIDANCE,
    PROMPT_OWNER_GUIDANCE, PROMPT_PROPOSAL_PACKET, PROMPT_ROUTE_BLOCKER_SUMMARY,
    PROMPT_ROUTE_READINESS_GUIDANCE, PROMPT_RUNTIME_CONTROL_GUIDANCE, PROMPT_STATUS_SUMMARY,
    PROMPT_STRATEGY_SELECTION_DISCUSSION, PROMPT_STRATEGY_SELECTION_GUIDANCE,
    PROMPT_STRATEGY_SELECTION_RECOVERY, ReadSessionResourceRequest, ReloadBundleRequest,
    RenderPromptRequest, SERVER_NAME, SessionInterpretationStatus, SessionProposalCompleteness,
    SessionProposalReadiness, SkillSessionResourceKind, TOOL_APPLY_ONBOARDING_ACTION,
    TOOL_APPLY_ROUTE_READINESS_ACTION, TOOL_BOOTSTRAP_INSTALL, TOOL_CLEAR_STOP,
    TOOL_EVALUATE_ROUTE_READINESS, TOOL_GENERATE_PROPOSAL_PACKET, TOOL_LOAD_BUNDLE,
    TOOL_RELOAD_BUNDLE, TOOL_RUNTIME_CLEAR_STOP, TOOL_RUNTIME_PAUSE, TOOL_RUNTIME_STOP,
    TOOL_STOP_SESSION, TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE, TOOL_STRATEGY_SELECTION_APPROVE,
    TOOL_STRATEGY_SELECTION_MATERIALIZE, TOOL_STRATEGY_SELECTION_REOPEN, session_uri_root,
};
use reqwest::Client;
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

const READY_ENTRY_SKILL_MD: &str = r#"---
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

const OWNER_SETUP_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
"#;

const RESOURCE_TEMPLATE_PROPOSAL: &str = "a2ex://skills/sessions/{session_id}/proposal";
const ONBOARDING_GUIDED_STATE_TEMPLATE: &str =
    "a2ex://onboarding/installs/{install_id}/guided_state";
const ONBOARDING_CHECKLIST_TEMPLATE: &str = "a2ex://onboarding/installs/{install_id}/checklist";
const ONBOARDING_DIAGNOSTICS_TEMPLATE: &str = "a2ex://onboarding/installs/{install_id}/diagnostics";
const RUNTIME_CONTROL_STATUS_TEMPLATE: &str = "a2ex://runtime/control/{install_id}/status";
const RUNTIME_CONTROL_FAILURES_TEMPLATE: &str = "a2ex://runtime/control/{install_id}/failures";
const ROUTE_READINESS_SUMMARY_TEMPLATE: &str =
    "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/summary";
const ROUTE_READINESS_PROGRESS_TEMPLATE: &str =
    "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/progress";
const ROUTE_READINESS_BLOCKERS_TEMPLATE: &str =
    "a2ex://readiness/routes/{install_id}/{proposal_id}/{route_id}/blockers";
const STRATEGY_SELECTION_SUMMARY_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/summary";
const STRATEGY_SELECTION_OVERRIDES_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/overrides";
const STRATEGY_SELECTION_APPROVAL_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/approval";
const STRATEGY_SELECTION_DIFF_TEMPLATE: &str =
    "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/diff";
const STRATEGY_SELECTION_APPROVAL_HISTORY_TEMPLATE: &str = "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/approval-history";
const STRATEGY_RUNTIME_ELIGIBILITY_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/eligibility";
const STRATEGY_RUNTIME_MONITORING_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/monitoring";
const STRATEGY_OPERATOR_REPORT_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/operator-report";
const STRATEGY_REPORT_WINDOW_TEMPLATE: &str = "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/report-window/{cursor}";
const STRATEGY_EXCEPTION_ROLLUP_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/exception-rollup";
const PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE: &str = "operator.strategy_operator_report_guidance";
const PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE: &str = "operator.strategy_report_window_guidance";

#[tokio::test]
async fn advertises_skill_tools_resources_and_prompts_as_separate_mcp_capabilities() {
    let server = A2exSkillMcpServer::new(Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("surface contract should initialize before runtime handlers exist");

    assert_eq!(capabilities.server_name, SERVER_NAME);
    assert_eq!(
        capabilities
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            TOOL_LOAD_BUNDLE,
            TOOL_RELOAD_BUNDLE,
            TOOL_GENERATE_PROPOSAL_PACKET,
            TOOL_STOP_SESSION,
            TOOL_CLEAR_STOP,
            TOOL_RUNTIME_STOP,
            TOOL_RUNTIME_PAUSE,
            TOOL_RUNTIME_CLEAR_STOP,
            TOOL_BOOTSTRAP_INSTALL,
            TOOL_APPLY_ONBOARDING_ACTION,
            TOOL_EVALUATE_ROUTE_READINESS,
            TOOL_APPLY_ROUTE_READINESS_ACTION,
            TOOL_STRATEGY_SELECTION_MATERIALIZE,
            TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE,
            TOOL_STRATEGY_SELECTION_APPROVE,
            TOOL_STRATEGY_SELECTION_REOPEN,
        ],
        "the MCP surface must keep proposal generation and strategy selection as separate first-class tools"
    );
    assert_eq!(
        capabilities
            .resources
            .iter()
            .map(|resource| resource.uri_template.as_str())
            .collect::<Vec<_>>(),
        vec![
            "a2ex://skills/sessions/{session_id}/status",
            "a2ex://skills/sessions/{session_id}/bundle",
            "a2ex://skills/sessions/{session_id}/interpretation",
            "a2ex://skills/sessions/{session_id}/blockers",
            "a2ex://skills/sessions/{session_id}/ambiguities",
            "a2ex://skills/sessions/{session_id}/provenance",
            "a2ex://skills/sessions/{session_id}/lifecycle",
            RESOURCE_TEMPLATE_PROPOSAL,
            "a2ex://skills/sessions/{session_id}/operator_state",
            "a2ex://skills/sessions/{session_id}/failures",
            ONBOARDING_GUIDED_STATE_TEMPLATE,
            ONBOARDING_CHECKLIST_TEMPLATE,
            ONBOARDING_DIAGNOSTICS_TEMPLATE,
            RUNTIME_CONTROL_STATUS_TEMPLATE,
            RUNTIME_CONTROL_FAILURES_TEMPLATE,
            ROUTE_READINESS_SUMMARY_TEMPLATE,
            ROUTE_READINESS_PROGRESS_TEMPLATE,
            ROUTE_READINESS_BLOCKERS_TEMPLATE,
            STRATEGY_SELECTION_SUMMARY_TEMPLATE,
            STRATEGY_SELECTION_OVERRIDES_TEMPLATE,
            STRATEGY_SELECTION_APPROVAL_TEMPLATE,
            STRATEGY_SELECTION_DIFF_TEMPLATE,
            STRATEGY_SELECTION_APPROVAL_HISTORY_TEMPLATE,
            STRATEGY_RUNTIME_ELIGIBILITY_TEMPLATE,
            STRATEGY_RUNTIME_MONITORING_TEMPLATE,
            STRATEGY_OPERATOR_REPORT_TEMPLATE,
            STRATEGY_REPORT_WINDOW_TEMPLATE,
            STRATEGY_EXCEPTION_ROLLUP_TEMPLATE,
        ],
        "R004/R005 require a readable MCP resource surface separate from tool results"
    );
    assert_eq!(
        capabilities
            .prompts
            .iter()
            .map(|prompt| prompt.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            PROMPT_STATUS_SUMMARY,
            PROMPT_OWNER_GUIDANCE,
            PROMPT_PROPOSAL_PACKET,
            PROMPT_OPERATOR_GUIDANCE,
            PROMPT_CURRENT_STEP_GUIDANCE,
            PROMPT_FAILURE_SUMMARY,
            PROMPT_ROUTE_READINESS_GUIDANCE,
            PROMPT_ROUTE_BLOCKER_SUMMARY,
            PROMPT_RUNTIME_CONTROL_GUIDANCE,
            PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE,
            PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE,
            PROMPT_STRATEGY_SELECTION_GUIDANCE,
            PROMPT_STRATEGY_SELECTION_DISCUSSION,
            PROMPT_STRATEGY_SELECTION_RECOVERY,
        ],
        "guided prompts must remain discoverable as prompts instead of hidden strings in tools"
    );
}

#[tokio::test]
async fn blocked_bundle_sessions_must_stay_readable_through_session_resources() {
    let harness = spawn_skill_bundle([(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(BLOCKED_ENTRY_SKILL_MD),
    )])
    .await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let server = A2exSkillMcpServer::new(Client::new());

    let first_load = server
        .load_bundle(LoadBundleRequest {
            entry_url: entry_url.clone(),
        })
        .await
        .expect(
            "skills.load_bundle should return session metadata even when the bundle is blocked",
        );

    assert_eq!(first_load.entry_url, entry_url);
    assert_eq!(first_load.status, SessionInterpretationStatus::Blocked);
    assert!(
        first_load.blocker_count >= 1,
        "blocked loads must advertise at least one typed blocker count"
    );
    assert_eq!(first_load.ambiguity_count, 0);
    assert_eq!(
        first_load.session_uri_root,
        session_uri_root(&first_load.session_id)
    );
    assert_eq!(
        first_load.resource_uris,
        vec![
            format!("{}/status", first_load.session_uri_root),
            format!("{}/bundle", first_load.session_uri_root),
            format!("{}/interpretation", first_load.session_uri_root),
            format!("{}/blockers", first_load.session_uri_root),
            format!("{}/ambiguities", first_load.session_uri_root),
            format!("{}/provenance", first_load.session_uri_root),
            format!("{}/lifecycle", first_load.session_uri_root),
            format!("{}/proposal", first_load.session_uri_root),
            format!("{}/operator_state", first_load.session_uri_root),
            format!("{}/failures", first_load.session_uri_root),
        ],
        "load responses must advertise where the durable session truth lives"
    );
    assert_eq!(
        first_load.prompt_names,
        vec![
            PROMPT_STATUS_SUMMARY.to_owned(),
            PROMPT_OWNER_GUIDANCE.to_owned(),
            PROMPT_PROPOSAL_PACKET.to_owned(),
            PROMPT_OPERATOR_GUIDANCE.to_owned(),
        ],
        "session metadata should advertise the reusable proposal prompt derived from the same session truth"
    );

    let reload = server
        .reload_bundle(ReloadBundleRequest {
            session_id: first_load.session_id.clone(),
            entry_url: entry_url.clone(),
        })
        .await
        .expect("skills.reload_bundle should preserve the existing session identity");
    assert_eq!(reload.session_id, first_load.session_id);
    assert_eq!(reload.session_uri_root, first_load.session_uri_root);

    let proposal_generation = server
        .generate_proposal_packet(GenerateProposalPacketRequest {
            session_id: first_load.session_id.clone(),
        })
        .await
        .expect("blocked sessions should still generate inspectable proposal metadata");
    assert_eq!(proposal_generation.session_id, first_load.session_id);
    assert_eq!(
        proposal_generation.proposal_uri,
        format!("{}/proposal", first_load.session_uri_root)
    );
    assert_eq!(proposal_generation.proposal_revision, 2);
    assert_eq!(
        proposal_generation.proposal_readiness,
        SessionProposalReadiness::Blocked
    );
    assert_eq!(
        proposal_generation.capital_profile_completeness,
        SessionProposalCompleteness::Blocked
    );
    assert_eq!(
        proposal_generation.cost_profile_completeness,
        SessionProposalCompleteness::Blocked
    );

    let status_resource = server
        .read_resource(ReadSessionResourceRequest {
            session_id: first_load.session_id.clone(),
            resource: SkillSessionResourceKind::Status,
        })
        .await
        .expect(
            "blocked session status should remain readable through MCP resources once the session registry exists",
        );
    assert_eq!(status_resource["session_id"], first_load.session_id);
    assert_eq!(status_resource["entry_url"], first_load.entry_url);
    assert_eq!(status_resource["status"], "blocked");
    assert_eq!(status_resource["blocker_count"], 1);
    assert_eq!(status_resource["last_operation"]["action"], "reload");
    assert_eq!(
        status_resource["proposal_uri"],
        proposal_generation.proposal_uri
    );
    assert_eq!(
        status_resource["proposal_revision"],
        proposal_generation.proposal_revision
    );
    assert_eq!(status_resource["proposal_readiness"], "blocked");

    let blockers_resource = server
        .read_resource(ReadSessionResourceRequest {
            session_id: first_load.session_id.clone(),
            resource: SkillSessionResourceKind::Blockers,
        })
        .await
        .expect(
            "blocked session blockers should be inspectable through typed resources, not only tool failure text",
        );
    assert_eq!(
        blockers_resource["blockers"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        blockers_resource["blockers"][0]["diagnostic_code"],
        "missing_required_document"
    );
    assert_eq!(
        blockers_resource["blockers"][0]["evidence"][0]["document_id"],
        "owner-setup"
    );

    let proposal_resource = server
        .read_resource(ReadSessionResourceRequest {
            session_id: first_load.session_id.clone(),
            resource: SkillSessionResourceKind::Proposal,
        })
        .await
        .expect("blocked session proposal state should remain readable through MCP resources");
    assert_eq!(
        proposal_resource["proposal_uri"],
        proposal_generation.proposal_uri
    );
    assert_eq!(
        proposal_resource["proposal_revision"],
        proposal_generation.proposal_revision
    );
    assert_eq!(
        proposal_resource["proposal"]["proposal_readiness"],
        "blocked"
    );
    assert_eq!(
        proposal_resource["proposal"]["blockers"][0]["diagnostic_code"],
        "missing_required_document"
    );

    let prompt = server
        .render_prompt(RenderPromptRequest {
            session_id: first_load.session_id.clone(),
            prompt_name: PROMPT_OWNER_GUIDANCE.to_owned(),
        })
        .await
        .expect("owner guidance prompt should render from the same blocked session state");
    assert_eq!(prompt.name, PROMPT_OWNER_GUIDANCE);
    assert_eq!(prompt.session_id, first_load.session_id);
    assert!(prompt.content.contains("missing_required_document"));
    assert!(prompt.content.contains("Inspect resources under"));
}

#[tokio::test]
async fn successful_bundle_sessions_expose_bundle_interpretation_and_status_prompts() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(READY_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD),
        ),
    ])
    .await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let server = A2exSkillMcpServer::new(Client::new());

    let load = server
        .load_bundle(LoadBundleRequest {
            entry_url: entry_url.clone(),
        })
        .await
        .expect("skills.load_bundle should succeed for a complete bundle");

    assert_eq!(load.entry_url, entry_url);
    assert_eq!(load.status, SessionInterpretationStatus::InterpretedReady);
    assert_eq!(load.blocker_count, 0);
    assert_eq!(load.ambiguity_count, 0);

    let proposal_generation = server
        .generate_proposal_packet(GenerateProposalPacketRequest {
            session_id: load.session_id.clone(),
        })
        .await
        .expect("ready sessions should generate stable proposal metadata");
    assert_eq!(proposal_generation.session_id, load.session_id);
    assert_eq!(
        proposal_generation.proposal_uri,
        format!("{}/proposal", load.session_uri_root)
    );
    assert_eq!(proposal_generation.proposal_revision, 1);
    assert_eq!(
        proposal_generation.proposal_readiness,
        SessionProposalReadiness::Ready
    );
    assert_eq!(
        proposal_generation.capital_profile_completeness,
        SessionProposalCompleteness::NotInBundleContract
    );
    assert_eq!(
        proposal_generation.cost_profile_completeness,
        SessionProposalCompleteness::NotInBundleContract
    );

    let bundle_resource = server
        .read_resource(ReadSessionResourceRequest {
            session_id: load.session_id.clone(),
            resource: SkillSessionResourceKind::Bundle,
        })
        .await
        .expect("complete bundle should expose parsed bundle resource");
    assert_eq!(
        bundle_resource["bundle"]["bundle_id"],
        "official.prediction-spread-arb"
    );
    assert_eq!(
        bundle_resource["bundle"]["documents"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        bundle_resource["diagnostics"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(bundle_resource["revision"], 1);
    assert_eq!(
        bundle_resource["lifecycle"]["current_bundle_version"], "2026.03.12",
        "bundle resource should expose bundle version metadata separately from the local session revision"
    );
    assert_eq!(
        bundle_resource["lifecycle"]["current_compatible_daemon"],
        ">=0.1.0"
    );

    let interpretation_resource = server
        .read_resource(ReadSessionResourceRequest {
            session_id: load.session_id.clone(),
            resource: SkillSessionResourceKind::Interpretation,
        })
        .await
        .expect("complete bundle should expose typed interpretation resource");
    assert_eq!(interpretation_resource["status"], "interpreted_ready");
    assert_eq!(
        interpretation_resource["plan_summary"]["bundle_id"],
        "official.prediction-spread-arb"
    );
    assert_eq!(
        interpretation_resource["owner_decisions"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    let proposal_resource = server
        .read_resource(ReadSessionResourceRequest {
            session_id: load.session_id.clone(),
            resource: SkillSessionResourceKind::Proposal,
        })
        .await
        .expect("complete bundle should expose typed proposal resource");
    assert_eq!(
        proposal_resource["proposal_uri"],
        proposal_generation.proposal_uri
    );
    assert_eq!(
        proposal_resource["proposal_revision"],
        proposal_generation.proposal_revision
    );
    assert_eq!(proposal_resource["proposal"]["proposal_readiness"], "ready");
    assert_eq!(
        proposal_resource["proposal"]["capital_profile"]["completeness"],
        "not_in_bundle_contract"
    );

    let proposal_prompt = server
        .render_prompt(RenderPromptRequest {
            session_id: load.session_id.clone(),
            prompt_name: PROMPT_PROPOSAL_PACKET.to_owned(),
        })
        .await
        .expect("proposal prompt should render from session-backed proposal truth");
    assert_eq!(proposal_prompt.name, PROMPT_PROPOSAL_PACKET);
    assert_eq!(proposal_prompt.session_id, load.session_id);
    assert_eq!(proposal_prompt.referenced_resources.len(), 3);
    assert!(
        proposal_prompt
            .content
            .contains(&proposal_generation.proposal_uri)
    );
    assert!(proposal_prompt.content.contains("proposal"));

    let prompt = server
        .render_prompt(RenderPromptRequest {
            session_id: load.session_id.clone(),
            prompt_name: PROMPT_STATUS_SUMMARY.to_owned(),
        })
        .await
        .expect("status summary prompt should render from session resources");
    assert_eq!(prompt.name, PROMPT_STATUS_SUMMARY);
    assert_eq!(prompt.session_id, load.session_id);
    assert_eq!(prompt.referenced_resources.len(), 2);
    assert!(prompt.content.contains("Blockers: 0"));
    assert!(prompt.content.contains(&load.session_id));
}

#[tokio::test]
async fn live_stdio_server_path_preserves_successful_reads_and_blocked_reload_diagnostics() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/ready/skill.md",
            BundleFixture::markdown(READY_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/ready/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD),
        ),
        (
            "/bundles/blocked/skill.md",
            BundleFixture::markdown(BLOCKED_ENTRY_SKILL_MD),
        ),
    ])
    .await;

    let ready_entry_url = harness.url("/bundles/ready/skill.md");
    let blocked_entry_url = harness.url("/bundles/blocked/skill.md");

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
    assert!(server_info.capabilities.tools.is_some());
    assert!(server_info.capabilities.resources.is_some());
    assert!(server_info.capabilities.prompts.is_some());

    let tools = client
        .list_all_tools()
        .await
        .expect("live server should list its tools");
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        vec![
            TOOL_LOAD_BUNDLE,
            TOOL_RELOAD_BUNDLE,
            TOOL_GENERATE_PROPOSAL_PACKET,
            TOOL_STOP_SESSION,
            TOOL_CLEAR_STOP,
            TOOL_RUNTIME_STOP,
            TOOL_RUNTIME_PAUSE,
            TOOL_RUNTIME_CLEAR_STOP,
            TOOL_BOOTSTRAP_INSTALL,
            TOOL_APPLY_ONBOARDING_ACTION,
            TOOL_EVALUATE_ROUTE_READINESS,
            TOOL_APPLY_ROUTE_READINESS_ACTION,
            TOOL_STRATEGY_SELECTION_MATERIALIZE,
            TOOL_STRATEGY_SELECTION_APPLY_OVERRIDE,
            TOOL_STRATEGY_SELECTION_APPROVE,
            TOOL_STRATEGY_SELECTION_REOPEN,
        ]
    );

    let resource_templates = client
        .list_resource_templates(None)
        .await
        .expect("live server should list its resource templates");
    assert_eq!(
        resource_templates
            .resource_templates
            .iter()
            .map(|template| template.uri_template.as_str())
            .collect::<Vec<_>>(),
        vec![
            "a2ex://skills/sessions/{session_id}/status",
            "a2ex://skills/sessions/{session_id}/bundle",
            "a2ex://skills/sessions/{session_id}/interpretation",
            "a2ex://skills/sessions/{session_id}/blockers",
            "a2ex://skills/sessions/{session_id}/ambiguities",
            "a2ex://skills/sessions/{session_id}/provenance",
            "a2ex://skills/sessions/{session_id}/lifecycle",
            RESOURCE_TEMPLATE_PROPOSAL,
            "a2ex://skills/sessions/{session_id}/operator_state",
            "a2ex://skills/sessions/{session_id}/failures",
            ONBOARDING_GUIDED_STATE_TEMPLATE,
            ONBOARDING_CHECKLIST_TEMPLATE,
            ONBOARDING_DIAGNOSTICS_TEMPLATE,
            RUNTIME_CONTROL_STATUS_TEMPLATE,
            RUNTIME_CONTROL_FAILURES_TEMPLATE,
            ROUTE_READINESS_SUMMARY_TEMPLATE,
            ROUTE_READINESS_PROGRESS_TEMPLATE,
            ROUTE_READINESS_BLOCKERS_TEMPLATE,
            STRATEGY_SELECTION_SUMMARY_TEMPLATE,
            STRATEGY_SELECTION_OVERRIDES_TEMPLATE,
            STRATEGY_SELECTION_APPROVAL_TEMPLATE,
            STRATEGY_SELECTION_DIFF_TEMPLATE,
            STRATEGY_SELECTION_APPROVAL_HISTORY_TEMPLATE,
            STRATEGY_RUNTIME_ELIGIBILITY_TEMPLATE,
            STRATEGY_RUNTIME_MONITORING_TEMPLATE,
            STRATEGY_OPERATOR_REPORT_TEMPLATE,
            STRATEGY_REPORT_WINDOW_TEMPLATE,
            STRATEGY_EXCEPTION_ROLLUP_TEMPLATE,
        ]
    );

    let prompts = client
        .list_prompts(None)
        .await
        .expect("live server should list its prompts");
    assert_eq!(
        prompts
            .prompts
            .iter()
            .map(|prompt| prompt.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            PROMPT_STATUS_SUMMARY,
            PROMPT_OWNER_GUIDANCE,
            PROMPT_PROPOSAL_PACKET,
            PROMPT_OPERATOR_GUIDANCE,
            PROMPT_CURRENT_STEP_GUIDANCE,
            PROMPT_FAILURE_SUMMARY,
            PROMPT_ROUTE_READINESS_GUIDANCE,
            PROMPT_ROUTE_BLOCKER_SUMMARY,
            PROMPT_RUNTIME_CONTROL_GUIDANCE,
            PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE,
            PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE,
            PROMPT_STRATEGY_SELECTION_GUIDANCE,
            PROMPT_STRATEGY_SELECTION_DISCUSSION,
            PROMPT_STRATEGY_SELECTION_RECOVERY,
        ]
    );
    assert!(prompts.prompts.iter().all(|prompt| {
        let expected_arguments = match prompt.name.as_str() {
            PROMPT_CURRENT_STEP_GUIDANCE
            | PROMPT_FAILURE_SUMMARY
            | PROMPT_RUNTIME_CONTROL_GUIDANCE => {
                vec![PROMPT_ARGUMENT_INSTALL_ID]
            }
            PROMPT_ROUTE_READINESS_GUIDANCE | PROMPT_ROUTE_BLOCKER_SUMMARY => vec![
                PROMPT_ARGUMENT_INSTALL_ID,
                PROMPT_ARGUMENT_PROPOSAL_ID,
                PROMPT_ARGUMENT_ROUTE_ID,
            ],
            PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE
            | PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE
            | PROMPT_STRATEGY_SELECTION_GUIDANCE
            | PROMPT_STRATEGY_SELECTION_DISCUSSION
            | PROMPT_STRATEGY_SELECTION_RECOVERY => vec![
                PROMPT_ARGUMENT_INSTALL_ID,
                PROMPT_ARGUMENT_PROPOSAL_ID,
                PROMPT_ARGUMENT_SELECTION_ID,
            ],
            _ => vec![PROMPT_ARGUMENT_SESSION_ID],
        };
        prompt.arguments.as_ref().map(|arguments| {
            arguments
                .iter()
                .map(|argument| argument.name.as_str())
                .collect::<Vec<_>>()
                == expected_arguments
                && arguments
                    .iter()
                    .all(|argument| argument.required == Some(true))
        }) == Some(true)
    }));

    let ready_load: LoadBundleResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_LOAD_BUNDLE).with_arguments(json_map([(
                    "entry_url",
                    Value::String(ready_entry_url.clone()),
                )])),
            )
            .await
            .expect("live server should load a complete bundle"),
    );
    assert_eq!(
        ready_load.status,
        SessionInterpretationStatus::InterpretedReady
    );
    assert_eq!(ready_load.blocker_count, 0);
    assert_eq!(
        ready_load.session_uri_root,
        session_uri_root(&ready_load.session_id)
    );
    assert_eq!(
        ready_load.prompt_names,
        vec![
            PROMPT_STATUS_SUMMARY.to_owned(),
            PROMPT_OWNER_GUIDANCE.to_owned(),
            PROMPT_PROPOSAL_PACKET.to_owned(),
            PROMPT_OPERATOR_GUIDANCE.to_owned(),
        ],
        "load tool responses should advertise the reusable proposal prompt"
    );
    assert!(
        ready_load
            .resource_uris
            .iter()
            .any(|uri| uri == &format!("{}/proposal", ready_load.session_uri_root)),
        "load tool responses should advertise the stable proposal resource uri"
    );

    let ready_generation: GenerateProposalPacketResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_GENERATE_PROPOSAL_PACKET).with_arguments(json_map(
                    [("session_id", Value::String(ready_load.session_id.clone()))],
                )),
            )
            .await
            .expect("live server should generate proposal metadata for ready sessions"),
    );
    assert_eq!(ready_generation.session_id, ready_load.session_id);
    assert_eq!(
        ready_generation.proposal_uri,
        format!("{}/proposal", ready_load.session_uri_root)
    );
    assert_eq!(
        ready_generation.proposal_readiness,
        SessionProposalReadiness::Ready
    );
    assert_eq!(
        ready_generation.capital_profile_completeness,
        SessionProposalCompleteness::NotInBundleContract
    );

    let ready_bundle_resource =
        read_resource_json(&client, format!("{}/bundle", ready_load.session_uri_root)).await;
    assert_eq!(
        ready_bundle_resource["bundle"]["bundle_id"],
        "official.prediction-spread-arb"
    );
    assert_eq!(
        ready_bundle_resource["bundle"]["documents"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let ready_proposal_resource =
        read_resource_json(&client, format!("{}/proposal", ready_load.session_uri_root)).await;
    assert_eq!(
        ready_proposal_resource["session_id"], ready_load.session_id,
        "proposal resource should be readable from the same stable session root"
    );
    assert_eq!(
        ready_proposal_resource["proposal_uri"], ready_generation.proposal_uri,
        "proposal resource should match the generation tool metadata"
    );
    assert_eq!(
        ready_proposal_resource["proposal_revision"], ready_generation.proposal_revision,
        "proposal resource should stay revision-coupled to the session snapshot"
    );
    assert_eq!(
        ready_proposal_resource["proposal"]["proposal_readiness"], "ready",
        "proposal resource should expose owner-facing readiness separate from interpretation output"
    );

    let ready_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_STATUS_SUMMARY).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(ready_load.session_id.clone()),
            )])),
        )
        .await
        .expect("live server should return the session status prompt");
    let ready_prompt_text = prompt_text(&ready_prompt);
    assert!(ready_prompt_text.contains("Blockers: 0"));
    assert!(ready_prompt_text.contains(&ready_load.session_id));
    assert_eq!(ready_prompt.messages.len(), 3);

    let proposal_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_PROPOSAL_PACKET).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(ready_load.session_id.clone()),
            )])),
        )
        .await
        .expect("live server should return the reusable proposal prompt");
    let proposal_prompt_text = prompt_text(&proposal_prompt);
    assert!(proposal_prompt_text.contains("proposal"));
    assert!(proposal_prompt_text.contains(&format!("{}/proposal", ready_load.session_uri_root)));

    let blocked_load: LoadBundleResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_LOAD_BUNDLE).with_arguments(json_map([(
                    "entry_url",
                    Value::String(blocked_entry_url.clone()),
                )])),
            )
            .await
            .expect("live server should return structured blocked-session metadata"),
    );
    assert_eq!(blocked_load.status, SessionInterpretationStatus::Blocked);
    assert_eq!(blocked_load.blocker_count, 1);

    let blocked_reload: LoadBundleResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_RELOAD_BUNDLE).with_arguments(json_map([
                    ("session_id", Value::String(blocked_load.session_id.clone())),
                    ("entry_url", Value::String(blocked_entry_url.clone())),
                ])),
            )
            .await
            .expect("live server should reload the blocked session without losing identity"),
    );
    assert_eq!(blocked_reload.session_id, blocked_load.session_id);
    assert_eq!(
        blocked_reload.session_uri_root,
        blocked_load.session_uri_root
    );

    let blocked_generation: GenerateProposalPacketResponse = decode_structured_tool_result(
        client
            .call_tool(
                CallToolRequestParams::new(TOOL_GENERATE_PROPOSAL_PACKET).with_arguments(json_map(
                    [("session_id", Value::String(blocked_load.session_id.clone()))],
                )),
            )
            .await
            .expect(
                "live server should generate inspectable proposal metadata for blocked sessions",
            ),
    );
    assert_eq!(blocked_generation.session_id, blocked_load.session_id);
    assert_eq!(blocked_generation.proposal_revision, 2);
    assert_eq!(
        blocked_generation.proposal_readiness,
        SessionProposalReadiness::Blocked
    );
    assert_eq!(
        blocked_generation.capital_profile_completeness,
        SessionProposalCompleteness::Blocked
    );

    let blocked_status_resource =
        read_resource_json(&client, format!("{}/status", blocked_load.session_uri_root)).await;
    assert_eq!(blocked_status_resource["status"], "blocked");
    assert_eq!(blocked_status_resource["blocker_count"], 1);
    assert_eq!(
        blocked_status_resource["last_operation"]["action"],
        "reload"
    );
    assert_eq!(
        blocked_status_resource["proposal_uri"],
        blocked_generation.proposal_uri
    );
    assert_eq!(
        blocked_status_resource["proposal_revision"],
        blocked_generation.proposal_revision
    );
    assert_eq!(blocked_status_resource["proposal_readiness"], "blocked");

    let blocked_blockers_resource = read_resource_json(
        &client,
        format!("{}/blockers", blocked_load.session_uri_root),
    )
    .await;
    assert_eq!(
        blocked_blockers_resource["blockers"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        blocked_blockers_resource["blockers"][0]["diagnostic_code"],
        "missing_required_document"
    );
    assert_eq!(
        blocked_blockers_resource["blockers"][0]["evidence"][0]["document_id"],
        "owner-setup"
    );

    let blocked_proposal_resource = read_resource_json(
        &client,
        format!("{}/proposal", blocked_load.session_uri_root),
    )
    .await;
    assert_eq!(
        blocked_proposal_resource["proposal_uri"],
        blocked_generation.proposal_uri
    );
    assert_eq!(
        blocked_proposal_resource["proposal_revision"],
        blocked_generation.proposal_revision
    );
    assert_eq!(
        blocked_proposal_resource["proposal"]["proposal_readiness"],
        "blocked"
    );
    assert_eq!(
        blocked_proposal_resource["proposal"]["blockers"][0]["diagnostic_code"],
        "missing_required_document"
    );

    let blocked_prompt = client
        .get_prompt(
            GetPromptRequestParams::new(PROMPT_OWNER_GUIDANCE).with_arguments(json_map([(
                PROMPT_ARGUMENT_SESSION_ID,
                Value::String(blocked_load.session_id.clone()),
            )])),
        )
        .await
        .expect("live server should return owner guidance for blocked sessions");
    let blocked_prompt_text = prompt_text(&blocked_prompt);
    assert!(blocked_prompt_text.contains("missing_required_document"));
    assert!(blocked_prompt_text.contains("Inspect resources under"));
    assert!(blocked_prompt_text.contains(&blocked_load.session_id));
    assert_eq!(blocked_prompt.messages.len(), 6);

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
