mod support;

use std::path::Path;

use a2ex_onboarding::{InspectStrategyRuntimeRequest, inspect_strategy_runtime_monitoring};
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

const EXPECTED_OPERATOR_REPORT_KIND: &str = "strategy_operator_report";

#[tokio::test]
async fn strategy_operator_report_contract_requires_named_strategy_shaped_report_fields_and_distinct_hold_failure_rejection_state()
 {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let fixture = prepare_approved_runtime_selection(
        &client,
        workspace_root.path(),
        &entry_url,
        "req-operator-report-contract-1",
        "intent-operator-report-contract-1",
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
            reservation_id: "reservation-operator-report-contract-exec".to_owned(),
            execution_id: "rebalance-operator-report-contract-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 310,
        })
        .await
        .expect("execution reservation should persist");

    let rebalance_command = runtime_service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-operator-report-contract".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-operator-report-contract".to_owned(),
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
            "reservation-operator-report-contract-exec",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:10:05Z",
        )
        .await
        .expect("rejected hedge path should still complete execution");

    let monitoring_after_failure = inspect_monitoring(workspace_root.path(), &fixture).await;
    let stopped = call_tool_json(
        &client,
        TOOL_RUNTIME_STOP,
        json_map([("install_id", Value::String(fixture.install_id.clone()))]),
    )
    .await;
    assert_eq!(stopped["control_mode"], "stopped");

    let monitoring_after_stop = inspect_monitoring(workspace_root.path(), &fixture).await;
    let monitoring_json = serde_json::to_value(&monitoring_after_stop)
        .expect("monitoring summary should remain serializable");

    let mut gaps = Vec::new();

    if monitoring_after_failure.last_runtime_failure.is_none() {
        gaps.push(
            "operator-report contract must preserve a distinct last_runtime_failure after runtime execution rejects a hedge"
                .to_owned(),
        );
    }
    if monitoring_after_failure.last_runtime_rejection.is_some() {
        gaps.push(
            "operator-report contract must keep runtime execution failure separate from runtime-control rejection before stop/pause is applied"
                .to_owned(),
        );
    }
    if monitoring_after_failure.handoff.hold_reason.is_some() {
        gaps.push(
            "operator-report contract must not flatten a pure runtime failure into hold_reason"
                .to_owned(),
        );
    }
    if monitoring_after_stop.handoff.hold_reason.is_none() {
        gaps.push(
            "operator-report contract must preserve an explicit hold_reason when runtime control stops autonomy"
                .to_owned(),
        );
    }
    if monitoring_after_stop
        .last_runtime_failure
        .as_ref()
        .map(|failure| failure.code.as_str())
        != Some("hedge_rejected")
    {
        gaps.push(
            "operator-report contract must keep hedge_rejected visible as last_runtime_failure after runtime.stop"
                .to_owned(),
        );
    }
    if monitoring_after_stop
        .last_runtime_rejection
        .as_ref()
        .map(|rejection| rejection.code.as_str())
        != Some("runtime_stopped")
    {
        gaps.push(
            "operator-report contract must keep runtime_stopped visible as last_runtime_rejection instead of collapsing it into hold or failure state"
                .to_owned(),
        );
    }

    for required_field in [
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
        if monitoring_json.get(required_field).is_none() {
            gaps.push(format!(
                "strategy-shaped operator-report must expose top-level field `{required_field}` instead of forcing agents to reconstruct it from adjacent runtime views"
            ));
        }
    }

    if monitoring_json.get("report_kind").and_then(Value::as_str)
        != Some(EXPECTED_OPERATOR_REPORT_KIND)
    {
        gaps.push(format!(
            "operator-report shape must identify itself with report_kind={EXPECTED_OPERATOR_REPORT_KIND} so downstream agents can lock the canonical contract"
        ));
    }

    assert!(
        gaps.is_empty(),
        "S01 direct operator-report contract missing named strategy-shaped report fields or distinct hold/failure/rejection semantics: {}",
        gaps.join("; ")
    );

    client
        .cancel()
        .await
        .expect("live client should shut down cleanly");
}

async fn inspect_monitoring(
    workspace_root: &Path,
    fixture: &support::ApprovedRuntimeSelectionFixture,
) -> a2ex_onboarding::StrategyRuntimeMonitoringSummary {
    inspect_strategy_runtime_monitoring(InspectStrategyRuntimeRequest {
        state_db_path: workspace_state_db_path(workspace_root),
        install_id: fixture.install_id.clone(),
        proposal_id: fixture.proposal_id.clone(),
        selection_id: fixture.selection_id.clone(),
    })
    .await
    .expect("runtime monitoring should remain inspectable from canonical local truth")
}
