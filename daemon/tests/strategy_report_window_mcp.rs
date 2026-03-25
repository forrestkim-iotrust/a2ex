mod support;

use a2ex_mcp::A2exSkillMcpServer;
use serde_json::Value;
use support::{
    PROMPT_ARGUMENT_INSTALL_ID, PROMPT_ARGUMENT_PROPOSAL_ID, PROMPT_ARGUMENT_SELECTION_ID,
    TOOL_RUNTIME_STOP, bootstrap_install_live, call_tool_json, json_map,
    prepare_approved_runtime_selection, prompt_text_result, read_resource_error,
    read_resource_json_result, ready_path_harness, spawn_live_client,
};
use tempfile::tempdir;

const RESOURCE_REPORT_WINDOW_TEMPLATE: &str = "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/report-window/{cursor}";
const RESOURCE_EXCEPTION_ROLLUP_TEMPLATE: &str =
    "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/exception-rollup";
const PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE: &str = "operator.strategy_report_window_guidance";
const EXPECTED_REPORT_WINDOW_KIND: &str = "strategy_report_window";
const EXPECTED_EXCEPTION_ROLLUP_KIND: &str = "strategy_exception_rollup";

fn report_window_changes_are_ordered(changes: &Value) -> bool {
    changes.as_array().is_some_and(|entries| {
        entries.windows(2).all(|pair| {
            pair[0]["observed_at"].as_str().unwrap_or_default()
                <= pair[1]["observed_at"].as_str().unwrap_or_default()
        })
    })
}

fn report_window_contains_change_kind(changes: &Value, expected_kind: &str) -> bool {
    changes.as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry["change_kind"].as_str() == Some(expected_kind))
    })
}

#[tokio::test]
async fn strategy_report_window_mcp_requires_discoverable_resources_one_guidance_prompt_and_reconnect_safe_rereads()
 {
    let server = A2exSkillMcpServer::new(reqwest::Client::new());
    let capabilities = server
        .initialize()
        .await
        .expect("MCP surface should initialize before S02 report-window handlers exist");
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
        "req-report-window-mcp-1",
        "intent-report-window-mcp-1",
    )
    .await;

    let report_window_uri = format!(
        "a2ex://strategy-runtime/selections/{}/{}/{}/report-window/bootstrap",
        fixture.install_id, fixture.proposal_id, fixture.selection_id
    );
    let exception_rollup_uri = format!(
        "a2ex://strategy-runtime/selections/{}/{}/{}/exception-rollup",
        fixture.install_id, fixture.proposal_id, fixture.selection_id
    );

    let stopped = call_tool_json(
        &first_client,
        TOOL_RUNTIME_STOP,
        json_map([("install_id", Value::String(fixture.install_id.clone()))]),
    )
    .await;
    assert_eq!(stopped["control_mode"], "stopped");

    let mut gaps = Vec::new();

    let report_window_count = advertised_resources
        .iter()
        .filter(|uri| **uri == RESOURCE_REPORT_WINDOW_TEMPLATE)
        .count();
    if report_window_count != 1 {
        gaps.push(format!(
            "initialize() must advertise exactly one report-window resource template {RESOURCE_REPORT_WINDOW_TEMPLATE}, found {report_window_count}"
        ));
    }

    let exception_rollup_count = advertised_resources
        .iter()
        .filter(|uri| **uri == RESOURCE_EXCEPTION_ROLLUP_TEMPLATE)
        .count();
    if exception_rollup_count != 1 {
        gaps.push(format!(
            "initialize() must advertise exactly one exception-rollup resource template {RESOURCE_EXCEPTION_ROLLUP_TEMPLATE}, found {exception_rollup_count}"
        ));
    }

    let prompt_count = advertised_prompts
        .iter()
        .filter(|name| **name == PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE)
        .count();
    if prompt_count != 1 {
        gaps.push(format!(
            "initialize() must advertise exactly one report-window guidance prompt {PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE}, found {prompt_count}"
        ));
    }

    if report_window_count == 1 {
        match read_resource_json_result(&first_client, report_window_uri.clone()).await {
            Ok(report_window) => {
                for required_field in [
                    "report_kind",
                    "cursor",
                    "window_start_cursor",
                    "window_end_cursor",
                    "window_limit",
                    "recent_changes",
                    "current_operator_report",
                    "exception_rollup",
                    "owner_action_needed_now",
                ] {
                    if report_window.get(required_field).is_none() {
                        gaps.push(format!(
                            "report-window resource must expose top-level `{required_field}` for reconnect-safe rereads"
                        ));
                    }
                }
                if report_window.get("report_kind").and_then(Value::as_str)
                    != Some(EXPECTED_REPORT_WINDOW_KIND)
                {
                    gaps.push(format!(
                        "report-window resource must identify itself with report_kind={EXPECTED_REPORT_WINDOW_KIND}"
                    ));
                }
                if !report_window_changes_are_ordered(&report_window["recent_changes"]) {
                    gaps.push(
                        "report-window resource must keep recent_changes ordered by observed_at so reconnect-safe rereads do not require local resorting"
                            .to_owned(),
                    );
                }
                if !report_window_contains_change_kind(
                    &report_window["recent_changes"],
                    "runtime_control_changed",
                ) {
                    gaps.push(
                        "report-window resource must surface the stop transition as a canonical recent change"
                            .to_owned(),
                    );
                }
                if report_window["current_operator_report"]["hold_reason"]
                    != "runtime_control_stopped"
                {
                    gaps.push(
                        "report-window resource must embed runtime_control_stopped in current_operator_report after runtime.stop"
                            .to_owned(),
                    );
                }
            }
            Err(error) => gaps.push(format!(
                "report-window resource must be readable from the shipped MCP surface after approval, got {error}"
            )),
        }
    }

    if exception_rollup_count == 1 {
        match read_resource_json_result(&first_client, exception_rollup_uri.clone()).await {
            Ok(exception_rollup) => {
                for required_field in [
                    "report_kind",
                    "owner_action_needed_now",
                    "urgency",
                    "recommended_operator_action",
                    "active_hold",
                    "last_runtime_failure",
                    "last_runtime_rejection",
                ] {
                    if exception_rollup.get(required_field).is_none() {
                        gaps.push(format!(
                            "exception-rollup resource must expose top-level `{required_field}` for direct MCP rereads"
                        ));
                    }
                }
                if exception_rollup.get("report_kind").and_then(Value::as_str)
                    != Some(EXPECTED_EXCEPTION_ROLLUP_KIND)
                {
                    gaps.push(format!(
                        "exception-rollup resource must identify itself with report_kind={EXPECTED_EXCEPTION_ROLLUP_KIND}"
                    ));
                }
            }
            Err(error) => gaps.push(format!(
                "exception-rollup resource must be readable from the shipped MCP surface after approval, got {error}"
            )),
        }
    }

    if prompt_count == 1 {
        match prompt_text_result(
            &first_client,
            PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE,
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
                if !prompt.contains(&report_window_uri) {
                    gaps.push(
                        "report-window guidance prompt must point agents at the canonical report-window URI with an explicit cursor"
                            .to_owned(),
                    );
                }
                if !prompt.contains(&exception_rollup_uri) {
                    gaps.push(
                        "report-window guidance prompt must point agents at the canonical exception-rollup URI"
                            .to_owned(),
                    );
                }
                if !prompt.contains("Do not rely on prior mutation receipts or session memory") {
                    gaps.push(
                        "report-window guidance prompt must explicitly reject session-memory shortcuts"
                            .to_owned(),
                    );
                }
            }
            Err(error) => gaps.push(format!(
                "report-window guidance prompt must render from canonical local truth, got {error}"
            )),
        }
    }

    first_client
        .cancel()
        .await
        .expect("first live stdio server should shut down cleanly");

    let reconnected_client = spawn_live_client().await;
    if report_window_count == 1 {
        let pre_reopen_error =
            read_resource_error(&reconnected_client, report_window_uri.clone()).await;
        if !(pre_reopen_error.contains("install") || pre_reopen_error.contains("locator")) {
            gaps.push(format!(
                "report-window resource must reject reconnect reads before bootstrap reopen repopulates the install locator, got {pre_reopen_error}"
            ));
        }
    }
    if exception_rollup_count == 1 {
        let pre_reopen_error =
            read_resource_error(&reconnected_client, exception_rollup_uri.clone()).await;
        if !(pre_reopen_error.contains("install") || pre_reopen_error.contains("locator")) {
            gaps.push(format!(
                "exception-rollup resource must reject reconnect reads before bootstrap reopen repopulates the install locator, got {pre_reopen_error}"
            ));
        }
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
            "bootstrap reopen must preserve install identity for reconnect-safe report-window and exception-rollup rereads"
                .to_owned(),
        );
    }

    if report_window_count == 1 {
        match read_resource_json_result(&reconnected_client, report_window_uri.clone()).await {
            Ok(report_window_after_reconnect) => {
                if report_window_after_reconnect.get("recent_changes").is_none()
                    || report_window_after_reconnect.get("exception_rollup").is_none()
                {
                    gaps.push(
                        "report-window rereads after reconnect must keep recent_changes and embedded exception_rollup visible on the canonical surface"
                            .to_owned(),
                    );
                }
                if !report_window_changes_are_ordered(
                    &report_window_after_reconnect["recent_changes"],
                ) {
                    gaps.push(
                        "report-window rereads after reconnect must keep recent_changes ordered by observed_at"
                            .to_owned(),
                    );
                }
                if !report_window_contains_change_kind(
                    &report_window_after_reconnect["recent_changes"],
                    "runtime_control_changed",
                ) {
                    gaps.push(
                        "report-window rereads after reconnect must preserve the stop transition in recent_changes"
                            .to_owned(),
                    );
                }
                if report_window_after_reconnect["current_operator_report"]["hold_reason"]
                    != "runtime_control_stopped"
                {
                    gaps.push(
                        "report-window rereads after reconnect must keep runtime_control_stopped visible in current_operator_report"
                            .to_owned(),
                    );
                }
            }
            Err(error) => gaps.push(format!(
                "report-window resource must stay readable after reconnect and bootstrap reopen, got {error}"
            )),
        }
    }

    if exception_rollup_count == 1 {
        match read_resource_json_result(&reconnected_client, exception_rollup_uri.clone()).await {
            Ok(exception_rollup_after_reconnect) => {
                if exception_rollup_after_reconnect
                    .get("last_runtime_failure")
                    .is_none()
                    || exception_rollup_after_reconnect
                        .get("last_runtime_rejection")
                        .is_none()
                {
                    gaps.push(
                        "exception-rollup rereads after reconnect must keep failure and rejection fields visible on the canonical surface"
                            .to_owned(),
                    );
                }
            }
            Err(error) => gaps.push(format!(
                "exception-rollup resource must stay readable after reconnect and bootstrap reopen, got {error}"
            )),
        }
    }

    reconnected_client
        .cancel()
        .await
        .expect("reconnected live stdio server should shut down cleanly");

    assert!(
        gaps.is_empty(),
        "S02 MCP report-window contract missing canonical resource/prompt discovery or reconnect-safe rereads: {}",
        gaps.join("; ")
    );
}
