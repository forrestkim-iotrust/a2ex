mod support;

use std::path::Path;

use a2ex_onboarding::{
    InspectStrategyReportWindowRequest, InspectStrategyRuntimeRequest,
    inspect_strategy_exception_rollup, inspect_strategy_report_window,
};
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::LocalPeerIdentity;
use a2ex_strategy_runtime::RuntimeWatcherState;
use serde_json::Value;
use support::{
    TOOL_RUNTIME_STOP, call_tool_json, json_map, prepare_approved_runtime_selection,
    ready_path_harness, register_strategy, rejected_runtime_service, spawn_live_client,
    workspace_state_db_path,
};
use tempfile::tempdir;

const EXPECTED_REPORT_WINDOW_KIND: &str = "strategy_report_window";
const EXPECTED_EXCEPTION_ROLLUP_KIND: &str = "strategy_exception_rollup";
const EXPECTED_OPERATOR_REPORT_KIND: &str = "strategy_operator_report";

#[tokio::test]
async fn strategy_report_window_contract_requires_explicit_cursor_bounds_ordered_changes_current_report_embedding_and_typed_exception_rollup()
 {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let fixture = prepare_approved_runtime_selection(
        &client,
        workspace_root.path(),
        &entry_url,
        "req-report-window-contract-1",
        "intent-report-window-contract-1",
    )
    .await;

    let runtime_service = rejected_runtime_service(workspace_root.path()).await;
    register_strategy(&runtime_service).await;

    let reservations =
        SqliteReservationManager::open(workspace_state_db_path(workspace_root.path()))
            .await
            .expect("reservations should open");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-report-window-contract-exec".to_owned(),
            execution_id: "rebalance-report-window-contract-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 310,
        })
        .await
        .expect("execution reservation should persist");

    let rebalance_command = runtime_service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-report-window-contract".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-report-window-contract".to_owned(),
                sampled_at: "2026-03-12T00:10:00Z".to_owned(),
            }],
            "2026-03-12T00:10:00Z",
        )
        .await
        .expect("runtime should emit a rebalance command before failure")
        .into_iter()
        .next()
        .expect("runtime should produce one command");

    runtime_service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command,
            "reservation-report-window-contract-exec",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:10:05Z",
        )
        .await
        .expect("rejected hedge path should still complete execution");

    let stopped = call_tool_json(
        &client,
        TOOL_RUNTIME_STOP,
        json_map([("install_id", Value::String(fixture.install_id.clone()))]),
    )
    .await;
    assert_eq!(stopped["control_mode"], "stopped");

    let report_window = inspect_report_window(workspace_root.path(), &fixture).await;
    let report_window_json =
        serde_json::to_value(&report_window).expect("report window should remain serializable");
    let exception_rollup = inspect_exception_rollup(workspace_root.path(), &fixture).await;
    let exception_rollup_json = serde_json::to_value(&exception_rollup)
        .expect("exception rollup should remain serializable");

    let mut gaps = Vec::new();

    if report_window_json
        .get("report_kind")
        .and_then(Value::as_str)
        != Some(EXPECTED_REPORT_WINDOW_KIND)
    {
        gaps.push(format!(
            "report-window contract must identify itself with report_kind={EXPECTED_REPORT_WINDOW_KIND}"
        ));
    }

    for required_field in [
        "cursor",
        "window_start_cursor",
        "window_end_cursor",
        "window_limit",
        "recent_changes",
        "current_operator_report",
        "exception_rollup",
        "owner_action_needed_now",
    ] {
        if report_window_json.get(required_field).is_none() {
            gaps.push(format!(
                "report-window contract must expose top-level field `{required_field}` instead of forcing agents to infer cursor/window state from prompt prose"
            ));
        }
    }

    match report_window_json.get("recent_changes").and_then(Value::as_array) {
        Some(changes) if !changes.is_empty() => {
            for required_field in [
                "cursor",
                "change_kind",
                "observed_at",
                "summary",
                "operator_impact",
            ] {
                if changes[0].get(required_field).is_none() {
                    gaps.push(format!(
                        "report-window recent_changes entries must expose `{required_field}` so agents can reread bounded canonical deltas without session memory"
                    ));
                }
            }
            let ordered = changes
                .windows(2)
                .all(|pair| pair[0]["observed_at"].as_str() <= pair[1]["observed_at"].as_str());
            if !ordered {
                gaps.push(
                    "report-window recent_changes must stay ordered by observed_at so reconnect-safe rereads can advance by cursor without local resorting"
                        .to_owned(),
                );
            }
        }
        Some(_) => gaps.push(
            "report-window contract must expose at least one change entry after approval, runtime failure, and stop transitions"
                .to_owned(),
        ),
        None => gaps.push(
            "report-window contract must expose recent_changes as an array of canonical change entries"
                .to_owned(),
        ),
    }

    match report_window_json.get("current_operator_report") {
        Some(current_operator_report)
            if current_operator_report
                .get("report_kind")
                .and_then(Value::as_str)
                == Some(EXPECTED_OPERATOR_REPORT_KIND) =>
        {
            for required_field in [
                "phase",
                "hold_reason",
                "control_mode",
                "owner_action_needed",
                "recommended_operator_action",
                "last_runtime_failure",
                "last_runtime_rejection",
            ] {
                if current_operator_report.get(required_field).is_none() {
                    gaps.push(format!(
                        "report-window current_operator_report must embed `{required_field}` from the canonical operator-report surface"
                    ));
                }
            }
        }
        Some(_) => gaps.push(format!(
            "report-window current_operator_report must embed the canonical operator report with report_kind={EXPECTED_OPERATOR_REPORT_KIND}"
        )),
        None => gaps.push(
            "report-window contract must embed current_operator_report so recent deltas and current truth stay coupled"
                .to_owned(),
        ),
    }

    if exception_rollup_json
        .get("report_kind")
        .and_then(Value::as_str)
        != Some(EXPECTED_EXCEPTION_ROLLUP_KIND)
    {
        gaps.push(format!(
            "exception-rollup contract must identify itself with report_kind={EXPECTED_EXCEPTION_ROLLUP_KIND}"
        ));
    }

    for required_field in [
        "owner_action_needed_now",
        "urgency",
        "recommended_operator_action",
        "active_hold",
        "last_runtime_failure",
        "last_runtime_rejection",
    ] {
        if exception_rollup_json.get(required_field).is_none() {
            gaps.push(format!(
                "exception-rollup contract must expose top-level `{required_field}` so holds, failures, and rejections stay distinct"
            ));
        }
    }

    if exception_rollup_json
        .get("active_hold")
        .and_then(|hold| hold.get("reason_code"))
        .is_none()
    {
        gaps.push(
            "exception-rollup contract must preserve typed active_hold.reason_code instead of flattening hold state into one summary string"
                .to_owned(),
        );
    }
    if exception_rollup_json
        .get("last_runtime_failure")
        .and_then(|failure| failure.get("code"))
        .is_none()
    {
        gaps.push(
            "exception-rollup contract must preserve typed last_runtime_failure.code after a rejected hedge"
                .to_owned(),
        );
    }
    if exception_rollup_json
        .get("last_runtime_rejection")
        .and_then(|rejection| rejection.get("code"))
        .is_none()
    {
        gaps.push(
            "exception-rollup contract must preserve typed last_runtime_rejection.code after runtime.stop"
                .to_owned(),
        );
    }

    assert!(
        gaps.is_empty(),
        "S02 direct report-window contract missing explicit cursor bounds, ordered changes, current operator report, or typed exception rollup fields: {}",
        gaps.join("; ")
    );

    client
        .cancel()
        .await
        .expect("live client should shut down cleanly");
}

async fn inspect_report_window(
    workspace_root: &Path,
    fixture: &support::ApprovedRuntimeSelectionFixture,
) -> a2ex_onboarding::StrategyReportWindow {
    inspect_strategy_report_window(InspectStrategyReportWindowRequest {
        state_db_path: workspace_state_db_path(workspace_root),
        install_id: fixture.install_id.clone(),
        proposal_id: fixture.proposal_id.clone(),
        selection_id: fixture.selection_id.clone(),
        cursor: "bootstrap".to_owned(),
        window_limit: 20,
    })
    .await
    .expect("report window should remain inspectable from canonical local truth")
}

async fn inspect_exception_rollup(
    workspace_root: &Path,
    fixture: &support::ApprovedRuntimeSelectionFixture,
) -> a2ex_onboarding::StrategyExceptionRollup {
    inspect_strategy_exception_rollup(InspectStrategyRuntimeRequest {
        state_db_path: workspace_state_db_path(workspace_root),
        install_id: fixture.install_id.clone(),
        proposal_id: fixture.proposal_id.clone(),
        selection_id: fixture.selection_id.clone(),
    })
    .await
    .expect("exception rollup should remain inspectable from canonical local truth")
}
