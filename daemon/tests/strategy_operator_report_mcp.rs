mod support;

use a2ex_mcp::A2exSkillMcpServer;
use serde_json::Value;
use support::{
    PROMPT_ARGUMENT_INSTALL_ID, PROMPT_ARGUMENT_PROPOSAL_ID, PROMPT_ARGUMENT_SELECTION_ID,
    bootstrap_install_live, json_map, prepare_approved_runtime_selection, prompt_text_result,
    read_resource_error, read_resource_json_result, ready_path_harness, spawn_live_client,
};
use tempfile::tempdir;

const RESOURCE_STRATEGY_OPERATOR_REPORT_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/operator-report";
const PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE: &str = "operator.strategy_operator_report_guidance";
const EXPECTED_OPERATOR_REPORT_KIND: &str = "strategy_operator_report";

#[tokio::test]
async fn strategy_operator_report_mcp_requires_one_resource_one_prompt_and_reconnect_safe_rereads()
{
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S01 operator-report handlers exist");
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

    let fixture = prepare_approved_runtime_selection(
        &first_client,
        workspace_root.path(),
        &entry_url,
        "req-operator-report-mcp-1",
        "intent-operator-report-mcp-1",
    )
    .await;

    let operator_report_uri = format!(
        "a2ex://strategy-runtime/selections/{}/{}/{}/operator-report",
        fixture.install_id, fixture.proposal_id, fixture.selection_id
    );

    let mut gaps = Vec::new();

    let resource_count = advertised_resources
        .iter()
        .filter(|uri| **uri == RESOURCE_STRATEGY_OPERATOR_REPORT_TEMPLATE)
        .count();
    if resource_count != 1 {
        gaps.push(format!(
            "initialize() must advertise exactly one operator-report resource template {RESOURCE_STRATEGY_OPERATOR_REPORT_TEMPLATE}, found {resource_count}"
        ));
    }

    let prompt_count = advertised_prompts
        .iter()
        .filter(|name| **name == PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE)
        .count();
    if prompt_count != 1 {
        gaps.push(format!(
            "initialize() must advertise exactly one operator-report guidance prompt {PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE}, found {prompt_count}"
        ));
    }

    match read_resource_json_result(&first_client, operator_report_uri.clone()).await {
        Ok(report) => {
            for required_field in [
                "report_kind",
                "phase",
                "last_action",
                "next_intended_action",
                "hold_reason",
                "control_mode",
                "reconciliation_evidence",
                "owner_action_needed",
                "recommended_operator_action",
                "last_runtime_failure",
                "last_runtime_rejection",
            ] {
                if report.get(required_field).is_none() {
                    gaps.push(format!(
                        "operator-report resource must expose top-level `{required_field}` for direct MCP rereads"
                    ));
                }
            }
            if report.get("report_kind").and_then(Value::as_str)
                != Some(EXPECTED_OPERATOR_REPORT_KIND)
            {
                gaps.push(format!(
                    "operator-report resource must identify itself with report_kind={EXPECTED_OPERATOR_REPORT_KIND}"
                ));
            }
        }
        Err(error) => gaps.push(format!(
            "operator-report resource must be readable from the shipped MCP surface after approval, got {error}"
        )),
    }

    if prompt_count == 1 {
        match prompt_text_result(
            &first_client,
            PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE,
            json_map([
                (
                    PROMPT_ARGUMENT_INSTALL_ID,
                    Value::String(fixture.install_id.clone()),
                ),
                (
                    PROMPT_ARGUMENT_PROPOSAL_ID,
                    Value::String(fixture.proposal_id.clone()),
                ),
                (
                    PROMPT_ARGUMENT_SELECTION_ID,
                    Value::String(fixture.selection_id.clone()),
                ),
            ]),
        )
        .await
        {
            Ok(prompt) => {
                if !prompt.contains(&operator_report_uri) {
                    gaps.push(
                        "operator-report guidance prompt must point agents at the canonical operator-report resource URI"
                            .to_owned(),
                    );
                }
                if !prompt.contains("Do not rely on prior mutation receipts or session memory") {
                    gaps.push(
                        "operator-report guidance prompt must explicitly reject session-memory shortcuts"
                            .to_owned(),
                    );
                }
            }
            Err(error) => gaps.push(format!(
                "operator-report guidance prompt must render from canonical local truth, got {error}"
            )),
        }
    }

    first_client
        .cancel()
        .await
        .expect("first live stdio server should shut down cleanly");

    let reconnected_client = spawn_live_client().await;
    if resource_count == 1 {
        let pre_reopen_error =
            read_resource_error(&reconnected_client, operator_report_uri.clone()).await;
        if !(pre_reopen_error.contains("install") || pre_reopen_error.contains("locator")) {
            gaps.push(format!(
                "operator-report resource must reject reconnect reads before bootstrap reopen repopulates the install locator, got {pre_reopen_error}"
            ));
        }

        let reopened = bootstrap_install_live(
            &reconnected_client,
            &entry_url,
            workspace_root.path(),
            Some(fixture.workspace_id.clone()),
            Some(fixture.install_id.clone()),
        )
        .await;
        if reopened["claim_disposition"] != "reopened" {
            gaps.push(
                "bootstrap reopen must preserve install identity for reconnect-safe operator-report rereads"
                    .to_owned(),
            );
        }

        match read_resource_json_result(&reconnected_client, operator_report_uri.clone()).await {
            Ok(report_after_reconnect) => {
                if report_after_reconnect.get("last_runtime_failure").is_none()
                    || report_after_reconnect.get("last_runtime_rejection").is_none()
                {
                    gaps.push(
                        "operator-report rereads after reconnect must keep failure and rejection fields visible on the canonical surface"
                            .to_owned(),
                    );
                }
            }
            Err(error) => gaps.push(format!(
                "operator-report resource must stay readable after reconnect and bootstrap reopen, got {error}"
            )),
        }
    } else {
        gaps.push(
            "reconnect-safe operator-report rereads cannot be verified until the shipped MCP surface advertises the canonical operator-report resource template"
                .to_owned(),
        );
    }

    reconnected_client
        .cancel()
        .await
        .expect("reconnected live stdio server should shut down cleanly");

    assert!(
        gaps.is_empty(),
        "S01 MCP operator-report contract missing canonical resource/prompt discovery or reconnect-safe rereads: {}",
        gaps.join("; ")
    );
}
