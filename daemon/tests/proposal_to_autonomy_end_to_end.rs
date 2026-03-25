mod support;

use a2ex_evm_adapter::SimulatedOutcome;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use serde_json::{Value, json};
use support::{
    PROMPT_ARGUMENT_INSTALL_ID, PROMPT_RUNTIME_CONTROL_GUIDANCE, TOOL_APPLY_ROUTE_READINESS_ACTION,
    TOOL_APPLY_STRATEGY_OVERRIDE, TOOL_APPROVE_STRATEGY_SELECTION, TOOL_EVALUATE_ROUTE_READINESS,
    TOOL_GENERATE_PROPOSAL_PACKET, TOOL_LOAD_BUNDLE, TOOL_MATERIALIZE_STRATEGY_SELECTION,
    TOOL_RUNTIME_CLEAR_STOP, TOOL_RUNTIME_PAUSE, TOOL_RUNTIME_STOP, TOOL_STRATEGY_SELECTION_REOPEN,
    apply_onboarding_action, bootstrap_install_live, call_tool_json, call_tool_json_result,
    expected_route_id, intent_request, json_map, prompt_text_result, read_resource_error,
    read_resource_json, ready_path_harness, register_strategy, routed_daemon_service,
    spawn_live_client, stateful_runtime_service,
};
use tempfile::tempdir;

#[tokio::test]
async fn proposal_to_autonomy_end_to_end_requires_same_identity_control_rereads_and_reapproval() {
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
    let request_id = "req-proposal-to-autonomy-1";
    let submit = routing_service
        .submit_intent(intent_request(request_id, "intent-proposal-to-autonomy-1"))
        .await
        .expect("submit intent should succeed");
    assert!(matches!(submit, a2ex_ipc::JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request(request_id)
        .await
        .expect("preview should succeed");
    let route_id = expected_route_id(&preview);

    let first_eval = call_tool_json(
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
    assert_eq!(first_eval["status"], "incomplete");

    let reservations =
        SqliteReservationManager::open(workspace_root.path().join(".a2ex-daemon/state.db"))
            .await
            .expect("reservations should open");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-proposal-to-autonomy-1".to_owned(),
            execution_id: request_id.to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("reservation should persist");

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

    let summary_uri = format!(
        "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/summary"
    );
    let diff_uri = format!(
        "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/diff"
    );
    let approval_history_uri = format!(
        "a2ex://strategy-selection/selections/{install_id}/{proposal_id}/{selection_id}/approval-history"
    );
    let monitoring_uri = format!(
        "a2ex://strategy-runtime/selections/{install_id}/{proposal_id}/{selection_id}/monitoring"
    );
    let runtime_status_uri = format!("a2ex://runtime/control/{install_id}/status");

    let summary_before_approval = read_resource_json(&client, summary_uri.clone()).await;
    assert_eq!(summary_before_approval["selection_id"], selection_id);

    let approved = call_tool_json(
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
                    "note": "approve assembled proposal-to-autonomy flow"
                }),
            ),
        ]),
    )
    .await;
    assert_eq!(approved["selection_id"], selection_id);

    let runtime_service =
        stateful_runtime_service(workspace_root.path(), SimulatedOutcome::Confirmed).await;
    register_strategy(&runtime_service).await;

    let monitoring_after_approval = read_resource_json(&client, monitoring_uri.clone()).await;
    let mut gaps = Vec::new();
    if monitoring_after_approval["install_id"] != install_id
        || monitoring_after_approval["proposal_id"] != proposal_id
        || monitoring_after_approval["selection_id"] != selection_id
    {
        gaps.push(
            "monitoring reread must retain canonical (install_id, proposal_id, selection_id) after approval"
                .to_owned(),
        );
    }

    let paused = call_tool_json(
        &client,
        TOOL_RUNTIME_PAUSE,
        json_map([("install_id", Value::String(install_id.clone()))]),
    )
    .await;
    if paused["control_mode"] != "paused" || paused["autonomy_eligibility"] != "blocked" {
        gaps.push(
            "runtime.pause must block new autonomous work through canonical control state"
                .to_owned(),
        );
    }
    let monitoring_after_pause = read_resource_json(&client, monitoring_uri.clone()).await;
    if monitoring_after_pause["hold_reason"] != "runtime_control_paused" {
        gaps.push(
            "monitoring reread must report runtime_control_paused distinctly from selection or failure diagnostics"
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
    if stopped["control_mode"] != "stopped" || stopped["autonomy_eligibility"] != "blocked" {
        gaps.push(
            "runtime.stop must block autonomous eligibility through canonical control state"
                .to_owned(),
        );
    }
    let monitoring_after_stop = read_resource_json(&client, monitoring_uri.clone()).await;
    if monitoring_after_stop["hold_reason"] != "runtime_control_stopped" {
        gaps.push(
            "monitoring reread must report runtime_control_stopped after stop on the approved selection"
                .to_owned(),
        );
    }

    let overridden = call_tool_json(
        &client,
        TOOL_APPLY_STRATEGY_OVERRIDE,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("selection_id", Value::String(selection_id.clone())),
            (
                "override",
                json!({
                    "key": "approve-max-spread-budget",
                    "value": { "resolution": "approved", "budget_bps": 25 },
                    "rationale": "readiness-sensitive override after autonomy handoff"
                }),
            ),
        ]),
    )
    .await;
    if overridden["selection_id"] != selection_id {
        gaps.push("readiness-sensitive override must preserve selection identity instead of minting a new selection".to_owned());
    }

    let diff_after_override = read_resource_json(&client, diff_uri.clone()).await;
    if diff_after_override["selection_id"] != selection_id {
        gaps.push(
            "diff reread must stay anchored to the original selection after override".to_owned(),
        );
    }
    if diff_after_override["approval_stale_reason"] != "readiness_sensitive_override" {
        gaps.push(
            "diff reread must explain reapproval through approval_stale_reason=readiness_sensitive_override"
                .to_owned(),
        );
    }

    client
        .cancel()
        .await
        .expect("first live client should shut down cleanly");

    let reconnected_client = spawn_live_client().await;
    let pre_reopen_monitoring_error =
        read_resource_error(&reconnected_client, monitoring_uri.clone()).await;
    if !(pre_reopen_monitoring_error.contains("install")
        || pre_reopen_monitoring_error.contains("locator"))
    {
        gaps.push(format!(
            "monitoring reads must fail before onboarding.bootstrap_install reopens the install, got {pre_reopen_monitoring_error}"
        ));
    }
    let pre_reopen_status_error =
        read_resource_error(&reconnected_client, runtime_status_uri).await;
    if !(pre_reopen_status_error.contains("install") || pre_reopen_status_error.contains("locator"))
    {
        gaps.push(format!(
            "runtime control reads must fail before onboarding.bootstrap_install reopens the install, got {pre_reopen_status_error}"
        ));
    }

    let reopened_install = bootstrap_install_live(
        &reconnected_client,
        &entry_url,
        workspace_root.path(),
        Some(workspace_id.clone()),
        Some(install_id.clone()),
    )
    .await;
    if reopened_install["claim_disposition"] != "reopened" {
        gaps.push("bootstrap reopen must preserve the same install identity".to_owned());
    }

    let monitoring_after_reconnect =
        read_resource_json(&reconnected_client, monitoring_uri.clone()).await;
    if monitoring_after_reconnect["install_id"] != install_id
        || monitoring_after_reconnect["proposal_id"] != proposal_id
        || monitoring_after_reconnect["selection_id"] != selection_id
    {
        gaps.push(
            "reconnect-safe monitoring reread must preserve canonical identity after bootstrap reopen"
                .to_owned(),
        );
    }
    if monitoring_after_reconnect["hold_reason"] != "approved_selection_revision_stale" {
        gaps.push(
            "after override and reconnect, monitoring must surface approved_selection_revision_stale until reopen/reapproval"
                .to_owned(),
        );
    }

    match call_tool_json_result(
        &reconnected_client,
        TOOL_STRATEGY_SELECTION_REOPEN,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("selection_id", Value::String(selection_id.clone())),
            (
                "reason",
                Value::String("operator reopened after reconnect before reapproval".to_owned()),
            ),
        ]),
    )
    .await
    {
        Ok(reopened) => {
            if reopened["selection_id"] != selection_id {
                gaps.push("strategy_selection.reopen must preserve the original selection identity".to_owned());
            }
        }
        Err(error) => gaps.push(format!(
            "strategy_selection.reopen must support reconnect-safe same-identity reopening, got {error}"
        )),
    }

    let approval_history_after_reopen =
        read_resource_json(&reconnected_client, approval_history_uri.clone()).await;
    let approved_event_count = approval_history_after_reopen["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter(|event| event["event_kind"] == "approved")
                .count()
        })
        .unwrap_or_default();
    if approved_event_count != 1 {
        gaps.push(format!(
            "approval-history must retain exactly one approval before reapproval, found {approved_event_count}"
        ));
    }

    let reapproved = match call_tool_json_result(
        &reconnected_client,
        TOOL_APPROVE_STRATEGY_SELECTION,
        json_map([
            ("install_id", Value::String(install_id.clone())),
            ("proposal_id", Value::String(proposal_id.clone())),
            ("selection_id", Value::String(selection_id.clone())),
            ("expected_selection_revision", Value::from(2)),
            (
                "approval",
                json!({
                    "approved_by": "owner",
                    "note": "reapprove reopened selection after reconnect"
                }),
            ),
        ]),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            gaps.push(format!(
                "assembled flow must allow same-identity reapproval after reconnect and reopen, got {error}"
            ));
            json!({})
        }
    };
    if reapproved.get("selection_id") != Some(&Value::String(selection_id.clone())) {
        gaps.push(
            "reapproval receipt must remain anchored to the original selection identity".to_owned(),
        );
    }

    let approval_history_after_reapproval =
        read_resource_json(&reconnected_client, approval_history_uri.clone()).await;
    let approved_event_count_after_reapproval = approval_history_after_reapproval["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter(|event| event["event_kind"] == "approved")
                .count()
        })
        .unwrap_or_default();
    if approved_event_count_after_reapproval < 2 {
        gaps.push(
            "approval-history reread must show both the original approval and same-identity reapproval"
                .to_owned(),
        );
    }

    let monitoring_after_reapproval =
        read_resource_json(&reconnected_client, monitoring_uri.clone()).await;
    if monitoring_after_reapproval["selection_id"] != selection_id {
        gaps.push(
            "monitoring reread after reapproval must still reference the original selection id"
                .to_owned(),
        );
    }
    if !monitoring_after_reapproval["hold_reason"].is_null()
        && monitoring_after_reapproval["hold_reason"] != "runtime_control_stopped"
    {
        gaps.push(
            "reapproval must clear revision-stale hold while preserving only active runtime control holds"
                .to_owned(),
        );
    }

    match prompt_text_result(
        &reconnected_client,
        PROMPT_RUNTIME_CONTROL_GUIDANCE,
        json_map([(
            PROMPT_ARGUMENT_INSTALL_ID,
            Value::String(install_id.clone()),
        )]),
    )
    .await
    {
        Ok(prompt) => {
            if !prompt.contains(TOOL_RUNTIME_CLEAR_STOP) || !prompt.contains(&install_id) {
                gaps.push(
                    "runtime.control_guidance must stay reconnect-safe and point at canonical clear-stop recovery after reapproval"
                        .to_owned(),
                );
            }
        }
        Err(error) => gaps.push(format!(
            "runtime.control_guidance should remain readable after reconnect/reapproval, got {error}"
        )),
    }

    reconnected_client
        .cancel()
        .await
        .expect("reconnected client should shut down cleanly");

    assert!(
        gaps.is_empty(),
        "S04 proposal-to-autonomy assembly contract missing stable identity, canonical rereads, runtime control transitions, or reconnect-safe reapproval: {}",
        gaps.join("; ")
    );
}
