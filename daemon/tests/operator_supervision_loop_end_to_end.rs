mod support;

use a2ex_signer_bridge::LocalPeerIdentity;
use a2ex_strategy_runtime::RuntimeWatcherState;
use serde_json::Value;
use support::{
    PROMPT_ARGUMENT_INSTALL_ID, PROMPT_ARGUMENT_PROPOSAL_ID, PROMPT_ARGUMENT_SELECTION_ID,
    TOOL_RUNTIME_CLEAR_STOP, TOOL_RUNTIME_STOP, bootstrap_install_live, call_tool_json, json_map,
    prepare_approved_runtime_selection, prompt_text_result, read_resource_error,
    read_resource_json, ready_path_harness, register_strategy, rejected_runtime_service,
    spawn_live_client, workspace_state_db_path,
};
use tempfile::tempdir;

const PROMPT_STRATEGY_OPERATOR_REPORT_GUIDANCE: &str = "operator.strategy_operator_report_guidance";
const PROMPT_STRATEGY_REPORT_WINDOW_GUIDANCE: &str = "operator.strategy_report_window_guidance";

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
async fn operator_supervision_loop_end_to_end_locks_the_live_mcp_supervision_contract_red_first() {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let first_client = spawn_live_client().await;

    let fixture = prepare_approved_runtime_selection(
        &first_client,
        workspace_root.path(),
        &entry_url,
        "req-operator-supervision-loop-1",
        "intent-operator-supervision-loop-1",
    )
    .await;

    let state_db_path = workspace_state_db_path(workspace_root.path());
    assert!(
        state_db_path.exists(),
        "live install bootstrap must create the canonical state.db before the assembled supervision loop runs"
    );

    let operator_report_uri = format!(
        "a2ex://strategy-runtime/selections/{}/{}/{}/operator-report",
        fixture.install_id, fixture.proposal_id, fixture.selection_id
    );
    let report_window_uri = format!(
        "a2ex://strategy-runtime/selections/{}/{}/{}/report-window/bootstrap",
        fixture.install_id, fixture.proposal_id, fixture.selection_id
    );
    let exception_rollup_uri = format!(
        "a2ex://strategy-runtime/selections/{}/{}/{}/exception-rollup",
        fixture.install_id, fixture.proposal_id, fixture.selection_id
    );
    let status_uri = format!("a2ex://runtime/control/{}/status", fixture.install_id);
    let failures_uri = format!("a2ex://runtime/control/{}/failures", fixture.install_id);

    let runtime_service = rejected_runtime_service(workspace_root.path()).await;
    register_strategy(&runtime_service).await;

    let rebalance_command = runtime_service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-operator-supervision-loop".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-operator-supervision-loop".to_owned(),
                sampled_at: "2026-03-12T00:10:00Z".to_owned(),
            }],
            "2026-03-12T00:10:00Z",
        )
        .await
        .expect("runtime should emit a rebalance command before the rejection path")
        .into_iter()
        .next()
        .expect("stateful runtime should produce a command");

    let reservations = a2ex_reservation::SqliteReservationManager::open(&state_db_path)
        .await
        .expect("reservations should open against the canonical state.db");
    a2ex_reservation::ReservationManager::hold(
        &reservations,
        a2ex_reservation::ReservationRequest {
            reservation_id: "reservation-operator-supervision-loop-exec".to_owned(),
            execution_id: "rebalance-operator-supervision-loop-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 310,
        },
    )
    .await
    .expect("execution reservation should persist before runtime-backed failure injection");

    runtime_service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command,
            "reservation-operator-supervision-loop-exec",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:10:05Z",
        )
        .await
        .expect(
            "rejected hedge path should complete its execution routine and persist failure truth",
        );

    let stop = call_tool_json(
        &first_client,
        TOOL_RUNTIME_STOP,
        json_map([("install_id", Value::String(fixture.install_id.clone()))]),
    )
    .await;
    assert_eq!(stop["control_mode"], "stopped");
    assert_eq!(stop["autonomy_eligibility"], "blocked");

    let operator_report_after_stop =
        read_resource_json(&first_client, operator_report_uri.clone()).await;
    let report_window_after_stop =
        read_resource_json(&first_client, report_window_uri.clone()).await;
    let exception_rollup_after_stop =
        read_resource_json(&first_client, exception_rollup_uri.clone()).await;
    let status_after_stop = read_resource_json(&first_client, status_uri.clone()).await;
    let failures_after_stop = read_resource_json(&first_client, failures_uri.clone()).await;

    let operator_prompt_after_stop = prompt_text_result(
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
    .expect("operator-report guidance prompt should render from canonical local truth");
    let report_window_prompt_after_stop = prompt_text_result(
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
    .expect("report-window guidance prompt should render from canonical local truth");
    let runtime_prompt_after_stop = prompt_text_result(
        &first_client,
        support::PROMPT_RUNTIME_CONTROL_GUIDANCE,
        json_map([(
            PROMPT_ARGUMENT_INSTALL_ID,
            Value::String(fixture.install_id.clone()),
        )]),
    )
    .await
    .expect("runtime control guidance prompt should render from canonical local truth");

    let mut gaps = Vec::new();

    for (surface, payload) in [
        ("operator-report", &operator_report_after_stop),
        (
            "report-window current_operator_report",
            &report_window_after_stop["current_operator_report"],
        ),
    ] {
        if payload["hold_reason"] != "runtime_control_stopped" {
            gaps.push(format!(
                "{surface} must expose hold_reason=runtime_control_stopped after runtime.stop"
            ));
        }
        if payload["last_runtime_failure"]["code"] != "hedge_rejected" {
            gaps.push(format!(
                "{surface} must preserve last_runtime_failure.code=hedge_rejected after the runtime-backed failure path"
            ));
        }
        if payload["last_runtime_rejection"]["code"] != "runtime_stopped" {
            gaps.push(format!(
                "{surface} must expose last_runtime_rejection.code=runtime_stopped distinct from the runtime failure"
            ));
        }
    }

    if !report_window_changes_are_ordered(&report_window_after_stop["recent_changes"]) {
        gaps.push(
            "report-window recent_changes must stay ordered by observed_at after runtime.stop so reconnect-safe rereads do not need session-local resorting"
                .to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_stop["recent_changes"],
        "execution_state_changed",
    ) {
        gaps.push(
            "report-window recent_changes must keep the runtime-backed execution failure visible after runtime.stop"
                .to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_stop["recent_changes"],
        "runtime_control_changed",
    ) {
        gaps.push(
            "report-window recent_changes must keep the runtime control stop transition visible after runtime.stop"
                .to_owned(),
        );
    }

    if exception_rollup_after_stop["active_hold"]["reason_code"] != "runtime_control_stopped" {
        gaps.push(
            "exception-rollup must expose active_hold.reason_code=runtime_control_stopped after runtime.stop"
                .to_owned(),
        );
    }
    if exception_rollup_after_stop["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "exception-rollup must preserve last_runtime_failure.code=hedge_rejected after the runtime-backed failure path"
                .to_owned(),
        );
    }
    if exception_rollup_after_stop["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "exception-rollup must expose last_runtime_rejection.code=runtime_stopped distinct from the runtime failure"
                .to_owned(),
        );
    }

    if status_after_stop["control_mode"] != "stopped"
        || status_after_stop["autonomy_eligibility"] != "blocked"
    {
        gaps.push(
            "runtime control status must expose stopped/blocked truth after runtime.stop"
                .to_owned(),
        );
    }
    if failures_after_stop["last_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "runtime control failures must preserve last_rejection.code=runtime_stopped after runtime.stop"
                .to_owned(),
        );
    }

    if !(operator_prompt_after_stop.contains(&operator_report_uri)
        && operator_prompt_after_stop.contains(&status_uri)
        && operator_prompt_after_stop.contains(&failures_uri)
        && operator_prompt_after_stop
            .contains("Do not rely on prior mutation receipts or session memory"))
    {
        gaps.push(
            "operator-report guidance must point back to canonical operator-report and runtime-control resources"
                .to_owned(),
        );
    }

    if !(report_window_prompt_after_stop.contains(&report_window_uri)
        && report_window_prompt_after_stop.contains(&exception_rollup_uri)
        && report_window_prompt_after_stop.contains(&operator_report_uri)
        && report_window_prompt_after_stop
            .contains("Do not rely on prior mutation receipts or session memory"))
    {
        gaps.push(
            "report-window guidance must point back to canonical report-window, exception-rollup, and operator-report resources"
                .to_owned(),
        );
    }

    if !(report_window_prompt_after_stop.contains(&status_uri)
        && report_window_prompt_after_stop.contains(&failures_uri)
        && report_window_prompt_after_stop.contains(support::PROMPT_RUNTIME_CONTROL_GUIDANCE)
        && report_window_prompt_after_stop.contains("runtime_stopped"))
    {
        gaps.push(
            "report-window guidance must hand the operator off into runtime control guidance and canonical status/failures resources once stop-derived holds are active"
                .to_owned(),
        );
    }

    if !(runtime_prompt_after_stop.contains(&status_uri)
        && runtime_prompt_after_stop.contains(&failures_uri)
        && runtime_prompt_after_stop.contains(&operator_report_uri)
        && runtime_prompt_after_stop.contains(&report_window_uri)
        && runtime_prompt_after_stop.contains(&exception_rollup_uri)
        && runtime_prompt_after_stop.contains("runtime_stopped"))
    {
        gaps.push(
            "runtime control guidance must point operators back to canonical operator-report, report-window, and exception-rollup rereads instead of isolating control recovery from the supervision reports"
                .to_owned(),
        );
    }

    first_client
        .cancel()
        .await
        .expect("first live stdio server should shut down cleanly");

    let reconnected_client = spawn_live_client().await;

    for uri in [
        operator_report_uri.clone(),
        report_window_uri.clone(),
        exception_rollup_uri.clone(),
        status_uri.clone(),
        failures_uri.clone(),
    ] {
        let error = read_resource_error(&reconnected_client, uri.clone()).await;
        if !(error.contains("install") || error.contains("locator")) {
            gaps.push(format!(
                "install-scoped reread {uri} must fail before onboarding.bootstrap_install reopens the install, got {error}"
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
            "bootstrap reopen must preserve the same workspace/install identity for assembled supervision rereads"
                .to_owned(),
        );
    }

    let operator_report_after_reconnect =
        read_resource_json(&reconnected_client, operator_report_uri.clone()).await;
    let report_window_after_reconnect =
        read_resource_json(&reconnected_client, report_window_uri.clone()).await;
    let exception_rollup_after_reconnect =
        read_resource_json(&reconnected_client, exception_rollup_uri.clone()).await;
    let failures_after_reconnect =
        read_resource_json(&reconnected_client, failures_uri.clone()).await;

    if operator_report_after_reconnect["last_runtime_failure"]["code"] != "hedge_rejected"
        || operator_report_after_reconnect["last_runtime_rejection"]["code"] != "runtime_stopped"
        || operator_report_after_reconnect["hold_reason"] != "runtime_control_stopped"
    {
        gaps.push(
            "operator-report must preserve distinct hold, failure, and rejection truth after reconnect"
                .to_owned(),
        );
    }
    if report_window_after_reconnect["current_operator_report"]["last_runtime_failure"]["code"]
        != "hedge_rejected"
        || report_window_after_reconnect["current_operator_report"]["last_runtime_rejection"]["code"]
            != "runtime_stopped"
        || report_window_after_reconnect["current_operator_report"]["hold_reason"]
            != "runtime_control_stopped"
    {
        gaps.push(
            "report-window must preserve distinct hold, failure, and rejection truth after reconnect"
                .to_owned(),
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
    if exception_rollup_after_reconnect["last_runtime_failure"]["code"] != "hedge_rejected"
        || exception_rollup_after_reconnect["last_runtime_rejection"]["code"] != "runtime_stopped"
        || exception_rollup_after_reconnect["active_hold"]["reason_code"]
            != "runtime_control_stopped"
    {
        gaps.push(
            "exception-rollup must preserve distinct hold, failure, and rejection truth after reconnect"
                .to_owned(),
        );
    }
    if failures_after_reconnect["last_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "runtime control failures must preserve last_rejection.code=runtime_stopped after reconnect"
                .to_owned(),
        );
    }

    let cleared = call_tool_json(
        &reconnected_client,
        TOOL_RUNTIME_CLEAR_STOP,
        json_map([("install_id", Value::String(fixture.install_id.clone()))]),
    )
    .await;
    assert_eq!(cleared["control_mode"], "active");
    assert_eq!(cleared["autonomy_eligibility"], "eligible");

    let operator_report_after_clear =
        read_resource_json(&reconnected_client, operator_report_uri.clone()).await;
    let report_window_after_clear =
        read_resource_json(&reconnected_client, report_window_uri.clone()).await;
    let exception_rollup_after_clear =
        read_resource_json(&reconnected_client, exception_rollup_uri.clone()).await;
    let failures_after_clear = read_resource_json(&reconnected_client, failures_uri.clone()).await;
    let runtime_prompt_after_clear = prompt_text_result(
        &reconnected_client,
        support::PROMPT_RUNTIME_CONTROL_GUIDANCE,
        json_map([(
            PROMPT_ARGUMENT_INSTALL_ID,
            Value::String(fixture.install_id.clone()),
        )]),
    )
    .await
    .expect("runtime control guidance prompt should render after clear-stop");

    if !report_window_changes_are_ordered(&report_window_after_clear["recent_changes"]) {
        gaps.push(
            "report-window recent_changes must stay ordered by observed_at after runtime.clear_stop"
                .to_owned(),
        );
    }
    if !report_window_contains_change_kind(
        &report_window_after_clear["recent_changes"],
        "runtime_control_changed",
    ) {
        gaps.push(
            "report-window recent_changes must preserve stop/clear-stop control transitions as canonical history after runtime.clear_stop"
                .to_owned(),
        );
    }

    for (surface, payload) in [
        ("operator-report", &operator_report_after_clear),
        (
            "report-window current_operator_report",
            &report_window_after_clear["current_operator_report"],
        ),
    ] {
        if !payload["hold_reason"].is_null() {
            gaps.push(format!(
                "{surface} must clear the active hold after runtime.clear_stop while preserving history"
            ));
        }
        if payload["last_runtime_failure"]["code"] != "hedge_rejected" {
            gaps.push(format!(
                "{surface} must preserve last_runtime_failure.code=hedge_rejected after runtime.clear_stop"
            ));
        }
        if payload["last_runtime_rejection"]["code"] != "runtime_stopped" {
            gaps.push(format!(
                "{surface} must preserve last_runtime_rejection.code=runtime_stopped as historical rejection after runtime.clear_stop"
            ));
        }
    }

    if !exception_rollup_after_clear["active_hold"].is_null() {
        gaps.push(
            "exception-rollup must clear active_hold after runtime.clear_stop while preserving failure and rejection history"
                .to_owned(),
        );
    }
    if exception_rollup_after_clear["last_runtime_failure"]["code"] != "hedge_rejected" {
        gaps.push(
            "exception-rollup must preserve last_runtime_failure.code=hedge_rejected after runtime.clear_stop"
                .to_owned(),
        );
    }
    if exception_rollup_after_clear["last_runtime_rejection"]["code"] != "runtime_stopped" {
        gaps.push(
            "exception-rollup must preserve last_runtime_rejection.code=runtime_stopped after runtime.clear_stop"
                .to_owned(),
        );
    }
    if failures_after_clear["control_mode"] != "active"
        || failures_after_clear["autonomy_eligibility"] != "eligible"
        || failures_after_clear["last_rejection"]["code"] != "runtime_stopped"
    {
        gaps.push(
            "runtime control failures must reflect active eligible state after runtime.clear_stop while preserving last rejection history"
                .to_owned(),
        );
    }
    if !(runtime_prompt_after_clear.contains("Autonomy eligibility: eligible")
        && runtime_prompt_after_clear.contains("runtime_stopped")
        && runtime_prompt_after_clear.contains(&operator_report_uri)
        && runtime_prompt_after_clear.contains(&report_window_uri)
        && runtime_prompt_after_clear.contains(&exception_rollup_uri))
    {
        gaps.push(
            "runtime control guidance must keep eligible recovery state and direct operators back to canonical supervision rereads after runtime.clear_stop"
                .to_owned(),
        );
    }

    reconnected_client
        .cancel()
        .await
        .expect("reconnected live stdio server should shut down cleanly");

    assert!(
        gaps.is_empty(),
        "S03 assembled supervision loop is still missing canonical prompt handoff or reconnect-safe distinct hold/failure/rejection rereads: {}",
        gaps.join("; ")
    );
}
