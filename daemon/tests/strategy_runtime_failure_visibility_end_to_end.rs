mod support;

use std::sync::Arc;

use a2ex_daemon::{DaemonConfig, DaemonService};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{HyperliquidAdapter, HyperliquidOrderStatus};
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::LocalPeerIdentity;
use a2ex_strategy_runtime::RuntimeWatcherState;
use serde_json::{Value, json};
use support::{
    AllowAllPolicy, RecordingSigner, SigningBridge, TOOL_APPLY_ROUTE_READINESS_ACTION,
    TOOL_APPROVE_STRATEGY_SELECTION, TOOL_EVALUATE_ROUTE_READINESS, TOOL_GENERATE_PROPOSAL_PACKET,
    TOOL_LOAD_BUNDLE, TOOL_MATERIALIZE_STRATEGY_SELECTION, TOOL_RUNTIME_CLEAR_STOP,
    TOOL_RUNTIME_PAUSE, TOOL_RUNTIME_STOP, apply_onboarding_action, bootstrap_install_live,
    call_tool_json, expected_route_id, intent_request, json_map, read_resource_error,
    read_resource_json, ready_path_harness, register_strategy, routed_daemon_service,
    spawn_live_client,
};
use tempfile::tempdir;

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
async fn strategy_runtime_failure_visibility_end_to_end_keeps_failures_distinct_from_holds() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let bootstrap =
        bootstrap_install_live(&client, &entry_url, workspace_root.path(), None, None).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id")
        .to_owned();
    let workspace_id = bootstrap["workspace_id"]
        .as_str()
        .expect("workspace id")
        .to_owned();

    apply_onboarding_action(
        &client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let onboarding_ready = apply_onboarding_action(
        &client,
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
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(entry_url.clone()))]),
    )
    .await;
    let proposal_id = load["session_id"].as_str().expect("proposal id").to_owned();
    let proposal = call_tool_json(
        &client,
        TOOL_GENERATE_PROPOSAL_PACKET,
        json_map([("session_id", Value::String(proposal_id.clone()))]),
    )
    .await;
    assert_eq!(proposal["proposal_readiness"], "ready");

    let routing_service = routed_daemon_service(workspace_root.path()).await;
    let request_id = "req-runtime-failure-1";
    let submit = routing_service
        .submit_intent(intent_request(request_id, "intent-runtime-failure-1"))
        .await
        .expect("submit intent should succeed");
    assert!(matches!(submit, a2ex_ipc::JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request(request_id)
        .await
        .expect("preview should succeed");
    let route_id = expected_route_id(&preview);

    let reservations =
        SqliteReservationManager::open(workspace_root.path().join(".a2ex-daemon/state.db"))
            .await
            .expect("reservations should open");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-runtime-failure-route".to_owned(),
            execution_id: request_id.to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("route reservation should persist");

    let second_eval = call_tool_json(
        &client,
        TOOL_EVALUATE_ROUTE_READINESS,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("route_id", Value::String(route_id.clone())),
            ("request_id", Value::String(request_id.to_owned())),
        ]),
    )
    .await;
    if second_eval["current_step_key"] != "satisfy_route_approvals" {
        call_tool_json(
            &client,
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
    }
    let ready_route = call_tool_json(
        &client,
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
        &client,
        TOOL_MATERIALIZE_STRATEGY_SELECTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
        ]),
    )
    .await;
    let selection_id = materialized["selection_id"]
        .as_str()
        .expect("selection id")
        .to_owned();

    call_tool_json(
        &client,
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
                    "note": "approve before runtime-backed failure injection"
                }),
            ),
        ]),
    )
    .await;

    let runtime_service = rejected_runtime_service(workspace_root.path()).await;
    register_strategy(&runtime_service).await;

    let monitoring_uri = format!(
        "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/monitoring"
    );
    let operator_report_uri = format!(
        "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/operator-report"
    );
    let report_window_uri = format!(
        "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/report-window/bootstrap"
    );
    let exception_rollup_uri = format!(
        "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/exception-rollup"
    );
    let failures_uri = format!("a2ex://runtime/control/{install_id}/failures");

    let monitoring_before_failure = read_resource_json(&client, monitoring_uri.clone()).await;
    let operator_report_before_failure =
        read_resource_json(&client, operator_report_uri.clone()).await;
    let report_window_before_failure = read_resource_json(&client, report_window_uri.clone()).await;
    let exception_rollup_before_failure =
        read_resource_json(&client, exception_rollup_uri.clone()).await;
    let failures_before_failure = read_resource_json(&client, failures_uri.clone()).await;
    let mut gaps = Vec::new();
    if monitoring_before_failure
        .get("last_runtime_failure")
        .is_none()
        || monitoring_before_failure
            .get("last_runtime_rejection")
            .is_none()
    {
        gaps.push(
            "monitoring rereads must expose stable last_runtime_failure and last_runtime_rejection fields even before a failure happens"
                .to_owned(),
        );
    }
    if operator_report_before_failure
        .get("last_runtime_failure")
        .is_none()
        || operator_report_before_failure
            .get("last_runtime_rejection")
            .is_none()
    {
        gaps.push(
            "operator-report rereads must expose stable last_runtime_failure and last_runtime_rejection fields even before a failure happens"
                .to_owned(),
        );
    }
    if report_window_before_failure.get("recent_changes").is_none()
        || report_window_before_failure
            .get("exception_rollup")
            .is_none()
    {
        gaps.push(
            "report-window rereads must expose stable recent_changes and embedded exception_rollup fields even before a failure happens"
                .to_owned(),
        );
    }
    if exception_rollup_before_failure
        .get("last_runtime_failure")
        .is_none()
        || exception_rollup_before_failure
            .get("last_runtime_rejection")
            .is_none()
    {
        gaps.push(
            "exception-rollup rereads must expose stable last_runtime_failure and last_runtime_rejection fields even before a failure happens"
                .to_owned(),
        );
    }
    if !failures_before_failure["last_rejection"].is_null() {
        gaps.push(
            "runtime control failures must keep last_rejection null before any runtime-control rejection occurs"
                .to_owned(),
        );
    }

    let rebalance_command = runtime_service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-failure".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-failure".to_owned(),
                sampled_at: "2026-03-12T00:10:00Z".to_owned(),
            }],
            "2026-03-12T00:10:00Z",
        )
        .await
        .expect("runtime should emit a rebalance command before failure")
        .into_iter()
        .next()
        .expect("stateful runtime should produce a command");

    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-runtime-failure-exec".to_owned(),
            execution_id: "rebalance-runtime-failure-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 310,
        })
        .await
        .expect("execution reservation should persist");

    runtime_service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command,
            "reservation-runtime-failure-exec",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:10:05Z",
        )
        .await
        .expect("runtime-backed rejected hedge path should still complete its execution routine");

    let monitoring_after_failure = read_resource_json(&client, monitoring_uri.clone()).await;
    let operator_report_after_failure =
        read_resource_json(&client, operator_report_uri.clone()).await;
    let report_window_after_failure = read_resource_json(&client, report_window_uri.clone()).await;
    let exception_rollup_after_failure =
        read_resource_json(&client, exception_rollup_uri.clone()).await;
    let failures_after_failure = read_resource_json(&client, failures_uri.clone()).await;
    if monitoring_after_failure["last_runtime_failure"].is_null() {
        gaps.push(
            "monitoring rereads must expose a real runtime failure after the rejected hedge path"
                .to_owned(),
        );
    }
    if monitoring_after_failure["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(format!(
            "runtime failure projection must expose hedge_rejected after the rejected hedge sync, found {}",
            monitoring_after_failure["last_runtime_failure"]["code"]
        ));
    }
    if !monitoring_after_failure["last_runtime_rejection"].is_null() {
        gaps.push(
            "a runtime hedge failure must not be conflated with a runtime-control rejection before any stop/pause hold is applied"
                .to_owned(),
        );
    }
    if !monitoring_after_failure["hold_reason"].is_null() {
        gaps.push(
            "a pure runtime failure must not invent a hold_reason before any explicit runtime-control hold exists"
                .to_owned(),
        );
    }
    if operator_report_after_failure["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "operator-report rereads must expose hedge_rejected after the rejected hedge sync"
                .to_owned(),
        );
    }
    if !operator_report_after_failure["last_runtime_rejection"].is_null() {
        gaps.push(
            "operator-report rereads must keep runtime hedge failures distinct from runtime-control rejections before any hold is applied"
                .to_owned(),
        );
    }
    if !operator_report_after_failure["hold_reason"].is_null() {
        gaps.push(
            "operator-report rereads must not invent a hold_reason before any explicit runtime-control hold exists"
                .to_owned(),
        );
    }
    if report_window_after_failure["current_operator_report"]["last_runtime_failure"]["code"]
        != "hedge_rejected"
    {
        gaps.push(
            "report-window rereads must embed the canonical operator report with hedge_rejected visible after the rejected hedge sync"
                .to_owned(),
        );
    }
    if !report_window_changes_are_ordered(&report_window_after_failure["recent_changes"]) {
        gaps.push(
            "report-window recent_changes must stay ordered by observed_at after a runtime execution failure"
                .to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_failure["recent_changes"],
        "execution_state_changed",
    ) {
        gaps.push(
            "report-window recent_changes must preserve the runtime-backed execution failure as a canonical change entry"
                .to_owned(),
        );
    }
    if !report_window_after_failure["current_operator_report"]["last_runtime_rejection"].is_null() {
        gaps.push(
            "report-window rereads must keep runtime hedge failures distinct from runtime-control rejections before any hold is applied"
                .to_owned(),
        );
    }
    if exception_rollup_after_failure["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "exception-rollup rereads must expose hedge_rejected after the rejected hedge sync"
                .to_owned(),
        );
    }
    if !exception_rollup_after_failure["last_runtime_rejection"].is_null() {
        gaps.push(
            "exception-rollup rereads must keep runtime hedge failures distinct from runtime-control rejections before any hold is applied"
                .to_owned(),
        );
    }
    if !exception_rollup_after_failure["active_hold"].is_null() {
        gaps.push(
            "exception-rollup rereads must not flatten a pure runtime failure into an active hold before runtime control changes"
                .to_owned(),
        );
    }
    if !failures_after_failure["last_rejection"].is_null() {
        gaps.push(
            "runtime control failures must not invent last_rejection history after a pure execution failure"
                .to_owned(),
        );
    }

    let paused = call_tool_json(
        &client,
        TOOL_RUNTIME_PAUSE,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    if paused["control_mode"] != "paused" {
        gaps.push(
            "runtime.pause must succeed before hold-vs-failure separation is asserted".to_owned(),
        );
    }

    let monitoring_after_pause = read_resource_json(&client, monitoring_uri.clone()).await;
    let operator_report_after_pause =
        read_resource_json(&client, operator_report_uri.clone()).await;
    let report_window_after_pause = read_resource_json(&client, report_window_uri.clone()).await;
    let exception_rollup_after_pause =
        read_resource_json(&client, exception_rollup_uri.clone()).await;
    if monitoring_after_pause["hold_reason"] != "runtime_control_paused" {
        gaps.push(
            "monitoring rereads must expose runtime_control_paused when pause is active".to_owned(),
        );
    }
    if monitoring_after_pause["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "pause holds must not erase the prior runtime failure diagnostic from monitoring"
                .to_owned(),
        );
    }
    if !monitoring_after_pause["last_runtime_rejection"].is_null() {
        gaps.push(
            "pause holds should remain distinct from runtime rejections until a blocked command actually occurs"
                .to_owned(),
        );
    }
    if operator_report_after_pause["hold_reason"] != "runtime_control_paused" {
        gaps.push(
            "operator-report rereads must expose runtime_control_paused when pause is active"
                .to_owned(),
        );
    }
    if operator_report_after_pause["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "operator-report rereads must preserve the prior runtime failure diagnostic while pause is active"
                .to_owned(),
        );
    }
    if !operator_report_after_pause["last_runtime_rejection"].is_null() {
        gaps.push(
            "operator-report rereads must keep pause holds distinct from runtime rejections until a blocked command actually occurs"
                .to_owned(),
        );
    }
    if report_window_after_pause["current_operator_report"]["hold_reason"]
        != "runtime_control_paused"
    {
        gaps.push(
            "report-window rereads must embed runtime_control_paused while pause is active"
                .to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_pause["recent_changes"],
        "runtime_control_changed",
    ) {
        gaps.push(
            "report-window recent_changes must record the pause transition distinctly from the prior execution failure"
                .to_owned(),
        );
    }
    if exception_rollup_after_pause["active_hold"]["reason_code"] != "runtime_control_paused" {
        gaps.push(
            "exception-rollup rereads must expose runtime_control_paused as a distinct active_hold while pause is active"
                .to_owned(),
        );
    }
    if exception_rollup_after_pause["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "exception-rollup rereads must preserve the prior runtime failure diagnostic while pause is active"
                .to_owned(),
        );
    }
    if !exception_rollup_after_pause["last_runtime_rejection"].is_null() {
        gaps.push(
            "exception-rollup rereads must keep pause holds distinct from runtime rejections until a blocked command actually occurs"
                .to_owned(),
        );
    }

    let clear_after_pause = call_tool_json(
        &client,
        TOOL_RUNTIME_CLEAR_STOP,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    if clear_after_pause["control_mode"] != "active" {
        gaps.push("runtime.clear_stop must restore active mode after pause".to_owned());
    }

    let stopped = call_tool_json(
        &client,
        TOOL_RUNTIME_STOP,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    if stopped["control_mode"] != "stopped" {
        gaps.push(
            "runtime.stop must succeed before rejection-vs-failure separation is asserted"
                .to_owned(),
        );
    }

    let monitoring_after_stop = read_resource_json(&client, monitoring_uri.clone()).await;
    let operator_report_after_stop = read_resource_json(&client, operator_report_uri.clone()).await;
    let report_window_after_stop = read_resource_json(&client, report_window_uri.clone()).await;
    let exception_rollup_after_stop =
        read_resource_json(&client, exception_rollup_uri.clone()).await;
    if monitoring_after_stop["hold_reason"] != "runtime_control_stopped" {
        gaps.push(
            "monitoring rereads must expose runtime_control_stopped while stop is active"
                .to_owned(),
        );
    }
    if monitoring_after_stop["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "runtime.stop holds must not overwrite the last runtime failure diagnostic".to_owned(),
        );
    }
    if monitoring_after_stop["last_runtime_rejection"].is_null() {
        gaps.push(
            "monitoring rereads must expose a distinct last_runtime_rejection alongside the stop hold"
                .to_owned(),
        );
    }
    if monitoring_after_stop["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "monitoring rereads must expose runtime_stopped as a rejection code distinct from the hedge failure"
                .to_owned(),
        );
    }
    if operator_report_after_stop["hold_reason"] != "runtime_control_stopped" {
        gaps.push(
            "operator-report rereads must expose runtime_control_stopped while stop is active"
                .to_owned(),
        );
    }
    if operator_report_after_stop["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "operator-report rereads must preserve the last runtime failure diagnostic while stop is active"
                .to_owned(),
        );
    }
    if operator_report_after_stop["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "operator-report rereads must expose runtime_stopped as a rejection code distinct from the hedge failure"
                .to_owned(),
        );
    }
    if report_window_after_stop["current_operator_report"]["hold_reason"]
        != "runtime_control_stopped"
    {
        gaps.push(
            "report-window rereads must keep the stop hold visible while stop is active".to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_stop["recent_changes"],
        "runtime_control_changed",
    ) {
        gaps.push(
            "report-window recent_changes must keep the stop transition visible as canonical history"
                .to_owned(),
        );
    }
    if report_window_after_stop["current_operator_report"]["last_runtime_rejection"]["code"]
        != "runtime_stopped"
    {
        gaps.push(
            "report-window rereads must embed runtime_stopped as a rejection code distinct from the hedge failure"
                .to_owned(),
        );
    }
    if exception_rollup_after_stop["active_hold"]["reason_code"] != "runtime_control_stopped" {
        gaps.push(
            "exception-rollup rereads must expose runtime_control_stopped as a distinct active_hold while stop is active"
                .to_owned(),
        );
    }
    if exception_rollup_after_stop["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "exception-rollup rereads must preserve hedge_rejected while stop is active".to_owned(),
        );
    }
    if exception_rollup_after_stop["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "exception-rollup rereads must expose runtime_stopped as a rejection code distinct from the hedge failure"
                .to_owned(),
        );
    }

    let failures_after_stop = read_resource_json(&client, failures_uri.clone()).await;
    if failures_after_stop["last_rejection"].is_null() {
        gaps.push(
            "runtime control failures resource must keep an explicit last_rejection object when stop is active"
                .to_owned(),
        );
    }
    if failures_after_stop["last_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "runtime control failures resource must keep stop-based rejection diagnostics distinct from runtime execution failure"
                .to_owned(),
        );
    }
    if failures_after_stop["last_rejection"]["code"]
        == monitoring_after_stop["last_runtime_failure"]["code"]
    {
        gaps.push(
            "runtime execution failure code and runtime-control rejection code must remain distinct on canonical rereads"
                .to_owned(),
        );
    }

    client
        .cancel()
        .await
        .expect("first live client should shut down cleanly");

    let reconnected_client = spawn_live_client().await;
    let pre_reopen_error = read_resource_error(&reconnected_client, monitoring_uri.clone()).await;
    if !(pre_reopen_error.contains("install") || pre_reopen_error.contains("locator")) {
        gaps.push(format!(
            "monitoring rereads must fail before reconnect bootstrap reopens the install, got {pre_reopen_error}"
        ));
    }
    let pre_reopen_operator_report_error =
        read_resource_error(&reconnected_client, operator_report_uri.clone()).await;
    if !(pre_reopen_operator_report_error.contains("install")
        || pre_reopen_operator_report_error.contains("locator"))
    {
        gaps.push(format!(
            "operator-report rereads must fail before reconnect bootstrap reopens the install, got {pre_reopen_operator_report_error}"
        ));
    }
    let pre_reopen_report_window_error =
        read_resource_error(&reconnected_client, report_window_uri.clone()).await;
    if !(pre_reopen_report_window_error.contains("install")
        || pre_reopen_report_window_error.contains("locator"))
    {
        gaps.push(format!(
            "report-window rereads must fail before reconnect bootstrap reopens the install, got {pre_reopen_report_window_error}"
        ));
    }
    let pre_reopen_exception_rollup_error =
        read_resource_error(&reconnected_client, exception_rollup_uri.clone()).await;
    if !(pre_reopen_exception_rollup_error.contains("install")
        || pre_reopen_exception_rollup_error.contains("locator"))
    {
        gaps.push(format!(
            "exception-rollup rereads must fail before reconnect bootstrap reopens the install, got {pre_reopen_exception_rollup_error}"
        ));
    }

    let reopened = bootstrap_install_live(
        &reconnected_client,
        &entry_url,
        workspace_root.path(),
        Some(workspace_id.clone()),
        Some(install_id.clone()),
    )
    .await;
    if reopened["claim_disposition"] != "reopened" {
        gaps.push(
            "bootstrap reopen must preserve install identity before failure rereads".to_owned(),
        );
    }

    let monitoring_after_reconnect =
        read_resource_json(&reconnected_client, monitoring_uri.clone()).await;
    let operator_report_after_reconnect =
        read_resource_json(&reconnected_client, operator_report_uri.clone()).await;
    let report_window_after_reconnect =
        read_resource_json(&reconnected_client, report_window_uri.clone()).await;
    let exception_rollup_after_reconnect =
        read_resource_json(&reconnected_client, exception_rollup_uri.clone()).await;
    if monitoring_after_reconnect["hold_reason"] != "runtime_control_stopped" {
        gaps.push(
            "stop hold must remain visible after reconnect instead of collapsing into failure state"
                .to_owned(),
        );
    }
    if monitoring_after_reconnect["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push("runtime failure diagnostic must remain visible after reconnect".to_owned());
    }
    if monitoring_after_reconnect["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "runtime rejection diagnostic must remain visible after reconnect and stay distinct from the execution failure"
                .to_owned(),
        );
    }
    if operator_report_after_reconnect["hold_reason"] != "runtime_control_stopped" {
        gaps.push(
            "operator-report rereads must keep the stop hold visible after reconnect instead of collapsing it into failure state"
                .to_owned(),
        );
    }
    if operator_report_after_reconnect["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "operator-report rereads must preserve the runtime failure diagnostic after reconnect"
                .to_owned(),
        );
    }
    if operator_report_after_reconnect["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "operator-report rereads must preserve the runtime rejection diagnostic after reconnect and keep it distinct from the execution failure"
                .to_owned(),
        );
    }
    if report_window_after_reconnect["current_operator_report"]["hold_reason"]
        != "runtime_control_stopped"
    {
        gaps.push(
            "report-window rereads must keep the stop hold visible after reconnect".to_owned(),
        );
    }
    if !report_window_changes_are_ordered(&report_window_after_reconnect["recent_changes"]) {
        gaps.push(
            "report-window recent_changes must stay ordered by observed_at after reconnect"
                .to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_reconnect["recent_changes"],
        "runtime_control_changed",
    ) {
        gaps.push(
            "report-window recent_changes must preserve the stop transition after reconnect"
                .to_owned(),
        );
    }
    if report_window_after_reconnect["current_operator_report"]["last_runtime_rejection"]["code"]
        != "runtime_stopped"
    {
        gaps.push(
            "report-window rereads must preserve runtime_stopped after reconnect and keep it distinct from the execution failure"
                .to_owned(),
        );
    }
    if exception_rollup_after_reconnect["active_hold"]["reason_code"] != "runtime_control_stopped" {
        gaps.push(
            "exception-rollup rereads must keep the stop hold visible after reconnect".to_owned(),
        );
    }
    if exception_rollup_after_reconnect["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "exception-rollup rereads must preserve hedge_rejected after reconnect".to_owned(),
        );
    }
    if exception_rollup_after_reconnect["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "exception-rollup rereads must preserve runtime_stopped after reconnect and keep it distinct from the execution failure"
                .to_owned(),
        );
    }

    let failures_after_reconnect =
        read_resource_json(&reconnected_client, failures_uri.clone()).await;
    if failures_after_reconnect["last_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "runtime control failures resource must preserve runtime_stopped rejection after reconnect"
                .to_owned(),
        );
    }

    let cleared = call_tool_json(
        &reconnected_client,
        TOOL_RUNTIME_CLEAR_STOP,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    if cleared["control_mode"] != "active" || cleared["autonomy_eligibility"] != "eligible" {
        gaps.push("runtime.clear_stop must restore active eligibility after reconnect".to_owned());
    }

    let monitoring_after_clear = read_resource_json(&reconnected_client, monitoring_uri).await;
    let operator_report_after_clear =
        read_resource_json(&reconnected_client, operator_report_uri).await;
    let report_window_after_clear =
        read_resource_json(&reconnected_client, report_window_uri).await;
    let exception_rollup_after_clear =
        read_resource_json(&reconnected_client, exception_rollup_uri).await;
    if !monitoring_after_clear["hold_reason"].is_null() {
        gaps.push(
            "clear_stop must remove runtime-control holds without erasing the runtime failure evidence"
                .to_owned(),
        );
    }
    if monitoring_after_clear["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "clear_stop must preserve the last runtime failure diagnostic after hold removal"
                .to_owned(),
        );
    }
    if monitoring_after_clear["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "clear_stop must preserve the prior stop rejection diagnostic as separate history"
                .to_owned(),
        );
    }
    if !operator_report_after_clear["hold_reason"].is_null() {
        gaps.push(
            "operator-report rereads must remove runtime-control holds after clear_stop without erasing the runtime failure evidence"
                .to_owned(),
        );
    }
    if operator_report_after_clear["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "operator-report rereads must preserve the last runtime failure diagnostic after clear_stop"
                .to_owned(),
        );
    }
    if operator_report_after_clear["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "operator-report rereads must preserve the prior stop rejection diagnostic as separate history after clear_stop"
                .to_owned(),
        );
    }
    if !report_window_after_clear["current_operator_report"]["hold_reason"].is_null() {
        gaps.push(
            "report-window rereads must remove runtime-control holds after clear_stop without erasing runtime failure evidence"
                .to_owned(),
        );
    }
    if !report_window_changes_are_ordered(&report_window_after_clear["recent_changes"]) {
        gaps.push(
            "report-window recent_changes must stay ordered by observed_at after clear_stop"
                .to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_clear["recent_changes"],
        "runtime_control_changed",
    ) {
        gaps.push(
            "report-window recent_changes must preserve stop/clear-stop control history after clear_stop"
                .to_owned(),
        );
    }
    if report_window_after_clear["current_operator_report"]["last_runtime_failure"]["code"]
        != "hedge_rejected"
    {
        gaps.push("report-window rereads must preserve hedge_rejected after clear_stop".to_owned());
    }
    if report_window_after_clear["current_operator_report"]["last_runtime_rejection"]["code"]
        != "runtime_stopped"
    {
        gaps.push(
            "report-window rereads must preserve runtime_stopped as separate history after clear_stop"
                .to_owned(),
        );
    }
    if !exception_rollup_after_clear["active_hold"].is_null() {
        gaps.push(
            "exception-rollup rereads must remove runtime-control holds after clear_stop without erasing runtime failure evidence"
                .to_owned(),
        );
    }
    if exception_rollup_after_clear["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "exception-rollup rereads must preserve hedge_rejected after clear_stop".to_owned(),
        );
    }
    if exception_rollup_after_clear["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "exception-rollup rereads must preserve runtime_stopped as separate history after clear_stop"
                .to_owned(),
        );
    }

    reconnected_client
        .cancel()
        .await
        .expect("reconnected client should shut down cleanly");

    assert!(
        gaps.is_empty(),
        "S04 runtime failure visibility contract missing distinct failure/rejection reporting or reconnect-safe rereads: {}",
        gaps.join("; ")
    );
}

async fn rejected_runtime_service(
    workspace_root: &std::path::Path,
) -> DaemonService<
    AllowAllPolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
    SimulatedEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    let harness = support::hyperliquid_harness::FakeHyperliquidTransport::default();
    harness.seed_open_orders(Vec::new());
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: "rejected".to_owned(),
        filled_size: "0.0".to_owned(),
    });
    harness.seed_user_fills(Vec::new());
    harness.seed_positions(Vec::new());

    DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        AllowAllPolicy,
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations clone"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge::default()),
            a2ex_signer_bridge::LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 10,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    )
}
