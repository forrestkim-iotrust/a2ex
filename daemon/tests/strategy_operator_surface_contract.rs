mod support;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::AcrossAdapter;
use a2ex_daemon::{DaemonConfig, DaemonService, ExecutionRequest, SignerHandoff};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_onboarding::{
    ApplyStrategySelectionOverride, ApplyStrategySelectionOverrideRequest,
    ApproveStrategySelectionRequest, GuidedOnboardingInspectionRequest,
    InspectStrategyRuntimeRequest, InspectStrategySelectionRequest, RouteReadinessAction,
    RouteReadinessActionRequest, RouteReadinessEvaluationRequest, StrategyRuntimePhase,
    StrategySelectionApprovalInput, apply_route_readiness_action,
    apply_strategy_selection_override, approve_strategy_selection, evaluate_route_readiness,
    inspect_guided_onboarding, inspect_strategy_runtime_monitoring, inspect_strategy_selection,
};
use a2ex_policy::{PolicyDecision, PolicyEvaluator, PolicyInput};
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
};
use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ResourceContents},
    transport::TokioChildProcess,
};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value, json};
use support::{
    across_harness::FakeAcrossTransport,
    hyperliquid_harness::FakeHyperliquidTransport,
    prediction_market_harness::FakePredictionMarketTransport,
    skill_bundle_harness::{BundleFixture, SkillBundleHarness, spawn_skill_bundle},
};
use tempfile::tempdir;

const TOOL_BOOTSTRAP_INSTALL: &str = "onboarding.bootstrap_install";
const TOOL_APPLY_ONBOARDING_ACTION: &str = "onboarding.apply_action";
const TOOL_LOAD_BUNDLE: &str = "skills.load_bundle";
const TOOL_GENERATE_PROPOSAL_PACKET: &str = "skills.generate_proposal_packet";

const STRATEGY_SELECTION_STREAM_TYPE: &str = "strategy_selection";
const STRATEGY_SELECTION_REOPENED_EVENT: &str = "strategy_selection_reopened";
const STRATEGY_RUNTIME_STREAM_TYPE: &str = "strategy_runtime_handoff";
const STRATEGY_RUNTIME_IDENTITY_REFRESHED_EVENT: &str = "strategy_runtime_identity_refreshed";
const STRATEGY_SELECTION_APPROVAL_HISTORY_TABLE: &str =
    "onboarding_strategy_selection_approval_history";

const READY_PATH_ENTRY_SKILL_MD: &str = r#"---
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

Track spread divergences after setup is complete.

# Owner Decisions

- Approve max spread budget.
"#;

const READY_PATH_OWNER_SETUP_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

#[derive(Default, Clone)]
struct RecordingSigner {
    handoffs: Arc<Mutex<Vec<String>>>,
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, request: &ExecutionRequest) {
        self.handoffs
            .lock()
            .expect("handoff lock")
            .push(request.action_kind.clone());
    }
}

struct ApprovingBridge;

#[async_trait]
impl SignerBridge for ApprovingBridge {
    async fn request_approval(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalResult, a2ex_signer_bridge::SignerBridgeError> {
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(request),
        })
    }
}

#[derive(Clone, Default)]
struct AllowAllPolicy;

impl PolicyEvaluator for AllowAllPolicy {
    fn evaluate(&self, _input: &PolicyInput) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

#[tokio::test]
async fn strategy_operator_surface_contract_requires_same_identity_reopen_diff_projection_and_runtime_identity_refresh()
 {
    let harness = ready_path_harness().await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let client = spawn_live_client().await;

    let bootstrap = bootstrap_install_live(&client, &entry_url, workspace_root.path()).await;
    let install_id = bootstrap["install_id"]
        .as_str()
        .expect("install id string")
        .to_owned();

    apply_action(
        &client,
        &install_id,
        json!({ "kind": "complete_step", "step_key": "POLYMARKET_API_KEY" }),
    )
    .await;
    let ready = apply_action(
        &client,
        &install_id,
        json!({
            "kind": "resolve_owner_decision",
            "step_key": "approve-max-spread-budget",
            "resolution": "approved"
        }),
    )
    .await;
    assert_eq!(ready["aggregate_status"], "ready");

    let inspection = inspect_guided_onboarding(GuidedOnboardingInspectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
    })
    .await
    .expect("ready install should remain inspectable through the direct onboarding seam");
    let handoff = inspection
        .proposal_handoff
        .expect("ready onboarding must expose a proposal handoff before strategy selection");

    let load = call_tool_json(
        &client,
        TOOL_LOAD_BUNDLE,
        json_map([("entry_url", Value::String(handoff.entry_url.to_string()))]),
    )
    .await;
    let proposal_id = load["session_id"]
        .as_str()
        .expect("session id string")
        .to_owned();
    let proposal = call_tool_json(
        &client,
        TOOL_GENERATE_PROPOSAL_PACKET,
        json_map([("session_id", Value::String(proposal_id.clone()))]),
    )
    .await;
    assert_eq!(proposal["proposal_readiness"], "ready");

    let selection_before_approval = inspect_strategy_selection(InspectStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
    })
    .await
    .expect("proposal generation should materialize a direct strategy selection record");

    let routing_service = routed_daemon_service(workspace_root.path()).await;
    let submit = routing_service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, JsonRpcResponse::Success(_)));
    let preview = routing_service
        .preview_intent_request("req-operator-surface-direct-1")
        .await
        .expect("preview builds");
    let route_id = expected_route_id(&preview);

    let first_eval = evaluate_route_readiness(RouteReadinessEvaluationRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        request_id: "req-operator-surface-direct-1".to_owned(),
    })
    .await
    .expect("real route-readiness flow should remain available for the operator-surface contract");
    assert_eq!(first_eval.identity.route_id, route_id);

    let reservations =
        SqliteReservationManager::open(workspace_root.path().join(".a2ex-daemon/state.db"))
            .await
            .expect("reservations open");
    reservations
        .hold(ReservationRequest {
            reservation_id: "reservation-operator-surface-direct-1".to_owned(),
            execution_id: "req-operator-surface-direct-1".to_owned(),
            asset: "USDC".to_owned(),
            amount: 3_000,
        })
        .await
        .expect("reservation evidence persists");

    let second_eval = evaluate_route_readiness(RouteReadinessEvaluationRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        request_id: "req-operator-surface-direct-1".to_owned(),
    })
    .await
    .expect("reservation-backed reevaluation should remain direct-inspectable");
    if second_eval.current_step_key.as_deref() != Some("satisfy_route_approvals") {
        apply_route_readiness_action(RouteReadinessActionRequest {
            state_db_path: state_db_path(workspace_root.path()),
            install_id: install_id.clone(),
            proposal_id: proposal_id.clone(),
            route_id: route_id.clone(),
            action: RouteReadinessAction::CompleteStep {
                step_key: "fund_route_capital".to_owned(),
            },
        })
        .await
        .expect("fund_route_capital should be completable once reservation evidence exists");
    }
    let ready_route = apply_route_readiness_action(RouteReadinessActionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        route_id: route_id.clone(),
        action: RouteReadinessAction::CompleteStep {
            step_key: "satisfy_route_approvals".to_owned(),
        },
    })
    .await
    .expect("satisfy_route_approvals should finish the canonical ready route");
    assert_eq!(ready_route.status.as_str(), "ready");

    let approved = approve_strategy_selection(ApproveStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        selection_id: selection_before_approval.summary.selection_id.clone(),
        expected_selection_revision: selection_before_approval.summary.selection_revision,
        approval: StrategySelectionApprovalInput {
            approved_by: "owner".to_owned(),
            note: Some("approve before exercising reopen/recovery coverage".to_owned()),
        },
    })
    .await
    .expect("direct approval should still work while S03 operator behavior is missing");

    let reopened_candidate =
        apply_strategy_selection_override(ApplyStrategySelectionOverrideRequest {
            state_db_path: state_db_path(workspace_root.path()),
            install_id: install_id.clone(),
            proposal_id: proposal_id.clone(),
            selection_id: approved.selection_id.clone(),
            override_record: ApplyStrategySelectionOverride {
                key: "approve-max-spread-budget".to_owned(),
                value: json!({ "resolution": "approved", "budget_bps": 25 }),
                rationale: "reopen the same approved selection after a readiness-sensitive change"
                    .to_owned(),
                provenance: Some(json!({ "source": "operator_reopen_request" })),
            },
        })
        .await
        .expect(
            "post-approval override should stay inspectable while reopen semantics are implemented",
        );

    let reread = inspect_strategy_selection(InspectStrategySelectionRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
    })
    .await
    .expect("strategy selection reread should stay available after reopen-worthy override");

    let runtime_service = stateful_runtime_service(workspace_root.path()).await;
    register_strategy(&runtime_service).await;

    let monitoring = inspect_strategy_runtime_monitoring(InspectStrategyRuntimeRequest {
        state_db_path: state_db_path(workspace_root.path()),
        install_id: install_id.clone(),
        proposal_id: proposal_id.clone(),
        selection_id: approved.selection_id.clone(),
    })
    .await
    .expect("runtime monitoring should remain inspectable at the direct seam");

    let connection = Connection::open(state_db_path(workspace_root.path()))
        .expect("state db should remain inspectable after direct operator flow");
    let tables = table_names(&connection);
    let inspection_json = serde_json::to_value(&reread)
        .expect("strategy selection inspection should stay serializable for contract assertions");
    let monitoring_json = serde_json::to_value(&monitoring)
        .expect("runtime monitoring should stay serializable for contract assertions");
    let mut gaps = Vec::new();

    if reread.summary.selection_id != approved.selection_id {
        gaps.push("reopen must keep the canonical selection_id stable instead of creating a duplicate selection identity".to_owned());
    }
    if inspection_json["summary"]["status"] != "reopened" {
        gaps.push(format!(
            "direct inspection must expose status=reopened after a readiness-sensitive post-approval change on the same selection identity, got {}",
            inspection_json["summary"]["status"]
        ));
    }
    if inspection_json["summary"]["reopened_from_revision"] != approved.selection_revision {
        gaps.push("direct inspection must expose reopened_from_revision matching the last approved revision".to_owned());
    }
    if inspection_json["summary"]["approval_stale"] != true {
        gaps.push("direct inspection must expose approval_stale=true after reopen-worthy override invalidates the approved revision".to_owned());
    }
    if inspection_json["summary"]["approval_stale_reason"] != "readiness_sensitive_override" {
        gaps.push("direct inspection must expose approval_stale_reason=readiness_sensitive_override for operator recovery guidance".to_owned());
    }
    if !inspection_json["approval_history"]
        .as_array()
        .is_some_and(|items| {
            items.iter().any(|item| {
                item["event_kind"] == "approved"
                    && item["selection_revision"] == approved.selection_revision
                    && item["approved_by"] == "owner"
            })
        })
    {
        gaps.push("direct inspection must expose durable approval_history with the prior approval provenance after reopen".to_owned());
    }
    if !inspection_json["approval_history"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["event_kind"] == "reopened"))
    {
        gaps.push("direct inspection must expose a reopened approval-history event on the same selection identity".to_owned());
    }
    if inspection_json["effective_diff"]["baseline_kind"] != "recommended" {
        gaps.push("direct inspection must expose effective_diff.baseline_kind=recommended for operator discussion".to_owned());
    }
    if !inspection_json["effective_diff"]["changed_override_keys"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == "approve-max-spread-budget"))
    {
        gaps.push("direct inspection must expose effective_diff.changed_override_keys for the current override delta".to_owned());
    }
    if inspection_json["effective_diff"]["readiness_stale"] != true {
        gaps.push("direct inspection must expose effective_diff.readiness_stale=true after the reopen-worthy override".to_owned());
    }
    if inspection_json["discussion"]["recommendation_basis"]["source_kind"] != "proposal_packet" {
        gaps.push("direct inspection must expose recommendation_basis inside a discussion block for reconnect-safe operator rationale review".to_owned());
    }

    if !tables.contains(STRATEGY_SELECTION_APPROVAL_HISTORY_TABLE) {
        gaps.push(format!(
            "missing canonical table {STRATEGY_SELECTION_APPROVAL_HISTORY_TABLE} for durable approval and reopen provenance"
        ));
    }

    let reopened_event: Option<String> = connection
        .query_row(
            "SELECT payload_json
             FROM event_journal
             WHERE stream_type = ?1 AND stream_id = ?2 AND event_type = ?3
             ORDER BY created_at DESC, event_id DESC
             LIMIT 1",
            [
                STRATEGY_SELECTION_STREAM_TYPE,
                approved.selection_id.as_str(),
                STRATEGY_SELECTION_REOPENED_EVENT,
            ],
            |row| row.get(0),
        )
        .optional()
        .expect("event journal query should remain inspectable");
    if reopened_event.is_none() {
        gaps.push("event_journal must record strategy_selection_reopened for same-identity reopen transitions".to_owned());
    }

    let runtime_identity_event: Option<String> = connection
        .query_row(
            "SELECT payload_json
             FROM event_journal
             WHERE stream_type = ?1 AND stream_id = ?2 AND event_type = ?3
             ORDER BY created_at DESC, event_id DESC
             LIMIT 1",
            [
                STRATEGY_RUNTIME_STREAM_TYPE,
                approved.selection_id.as_str(),
                STRATEGY_RUNTIME_IDENTITY_REFRESHED_EVENT,
            ],
            |row| row.get(0),
        )
        .optional()
        .expect("runtime identity refresh event query should remain inspectable");
    if runtime_identity_event.is_none() {
        gaps.push("event_journal must record strategy_runtime_identity_refreshed once runtime monitoring resolves a real strategy_id".to_owned());
    }

    if monitoring_json["strategy_id"] != "strategy-lp-1" {
        gaps.push("direct runtime monitoring must refresh handoff.strategy_id after a real runtime identity exists".to_owned());
    }
    if monitoring.current_phase == StrategyRuntimePhase::AwaitingRuntimeIdentity {
        gaps.push("direct runtime monitoring must leave awaiting_runtime_identity once a real strategy_id is known".to_owned());
    }
    if monitoring_json["runtime_identity_refreshed_at"].is_null() {
        gaps.push("direct runtime monitoring must expose runtime_identity_refreshed_at for reconnect-safe freshness checks".to_owned());
    }
    if monitoring_json["runtime_identity_source"] != "strategy_runtime" {
        gaps.push("direct runtime monitoring must expose runtime_identity_source=strategy_runtime once the join succeeds".to_owned());
    }
    if monitoring_json["hold_reason"] != "approved_selection_revision_stale" {
        gaps.push(format!(
            "direct runtime monitoring must report approved_selection_revision_stale after reopen-worthy override, got {}",
            monitoring_json["hold_reason"]
        ));
    }
    if monitoring_json["last_operator_guidance"]["recommended_action"] != "reapprove_selection" {
        gaps.push("direct runtime monitoring must expose last_operator_guidance.recommended_action=reapprove_selection after reopen".to_owned());
    }
    if reopened_candidate.summary.selection_id != approved.selection_id {
        gaps.push(
            "reopen-worthy override mutation must keep the same selection identity stable"
                .to_owned(),
        );
    }

    assert!(
        gaps.is_empty(),
        "S03 direct operator-surface contract missing same-identity reopen, durable approval history, effective diff, or runtime-identity-backed monitoring: {}",
        gaps.join("; ")
    );

    client
        .cancel()
        .await
        .expect("live stdio server should shut down cleanly");
}

async fn ready_path_harness() -> SkillBundleHarness {
    spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(READY_PATH_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(READY_PATH_OWNER_SETUP_MD),
        ),
    ])
    .await
}

async fn routed_daemon_service(
    workspace_root: &Path,
) -> DaemonService<
    a2ex_policy::BaselinePolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
    a2ex_evm_adapter::NoopEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    let across = FakeAcrossTransport::default();
    let prediction = FakePredictionMarketTransport::default();
    let hyperliquid = FakeHyperliquidTransport::default();
    DaemonService::from_config_with_multi_venue_adapters(
        &config,
        a2ex_policy::BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        a2ex_evm_adapter::NoopEvmAdapter,
        AcrossAdapter::with_transport(across.transport(), 0),
        PredictionMarketAdapter::with_transport(prediction.transport()),
        HyperliquidAdapter::with_transport(hyperliquid.transport(), 0),
    )
}

async fn stateful_runtime_service(
    workspace_root: &Path,
) -> DaemonService<
    AllowAllPolicy,
    SqliteReservationManager,
    RecordingSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
    SimulatedEvmAdapter,
> {
    let config = DaemonConfig::for_data_dir(workspace_root.join(".a2ex-daemon"));
    let harness = seeded_hyperliquid_harness();
    DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        AllowAllPolicy,
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations clone"),
        Arc::new(RecordingSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(ApprovingBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 10,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    )
}

fn seeded_hyperliquid_harness() -> FakeHyperliquidTransport {
    let harness = FakeHyperliquidTransport::default();
    harness.seed_open_orders(vec![HyperliquidOpenOrder {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        is_buy: false,
        price: "2412.7".to_owned(),
        size: "0.5".to_owned(),
        reduce_only: false,
        status: "resting".to_owned(),
        client_order_id: Some("hl-strategy-lp-1-1".to_owned()),
    }]);
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: "filled".to_owned(),
        filled_size: "0.5".to_owned(),
    });
    harness.seed_user_fills(vec![HyperliquidUserFill {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "0.5".to_owned(),
        price: "2412.7".to_owned(),
        side: "sell".to_owned(),
        filled_at: "2026-03-12T00:10:01Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "-0.5".to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);
    harness
}

async fn register_strategy(
    service: &DaemonService<
        impl PolicyEvaluator,
        SqliteReservationManager,
        RecordingSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<ApprovingBridge>,
        SimulatedEvmAdapter,
    >,
) {
    let response: JsonRpcResponse<a2ex_daemon::StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
}

fn strategy_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-strategy-operator-surface-direct",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-operator-surface-direct",
            "request_kind": "strategy",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:09:00Z",
            "payload": {
                "strategy_id": "strategy-lp-1",
                "strategy_type": "stateful_hedge",
                "watchers": [{"watcher_type": "lp_position", "source": "local", "target": "TOKEN/USDT"}],
                "trigger_rules": [{"trigger_type": "drift_threshold", "metric": "delta_exposure_pct", "operator": ">", "value": "0.02", "cooldown_sec": 10}],
                "calculation_model": {"model_type": "delta_neutral_lp", "inputs": ["lp_token_balance"]},
                "action_templates": [{"action_type": "adjust_hedge", "venue": "hyperliquid", "instrument": "TOKEN-PERP", "target": "delta_neutral"}],
                "constraints": {"min_order_usd": 100, "max_slippage_bps": 40, "max_rebalances_per_hour": 60},
                "unwind_rules": [{"condition": "manual_stop"}]
            },
            "rationale": {"summary": "Keep delta neutral.", "main_risks": []},
            "execution_preferences": {"preview_only": false, "allow_fast_path": false}
        }),
    )
}

fn state_db_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".a2ex-daemon/state.db")
}

fn table_names(connection: &Connection) -> HashSet<String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .expect("sqlite_master query should prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("sqlite_master query should run");
    rows.collect::<Result<HashSet<_>, _>>()
        .expect("table names should collect")
}

fn expected_route_id(preview: &a2ex_daemon::IntentPreviewResponse) -> String {
    let adapters = preview
        .plan_preview
        .as_ref()
        .map(|plan| {
            plan.steps
                .iter()
                .map(|step| step.adapter.as_str())
                .collect::<Vec<_>>()
                .join(":")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "no_steps".to_owned());
    format!("{:?}:{adapters}", preview.route.route).to_lowercase()
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

async fn bootstrap_install_live(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_url: &str,
    workspace_root: &Path,
) -> Value {
    call_tool_json(
        client,
        TOOL_BOOTSTRAP_INSTALL,
        json_map([
            ("install_url", Value::String(install_url.to_owned())),
            (
                "workspace_root",
                Value::String(workspace_root.display().to_string()),
            ),
        ]),
    )
    .await
}

async fn apply_action(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    install_id: &str,
    action: Value,
) -> Value {
    call_tool_json(
        client,
        TOOL_APPLY_ONBOARDING_ACTION,
        json_map([
            ("install_id", Value::String(install_id.to_owned())),
            ("action", action),
        ]),
    )
    .await
}

async fn call_tool_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool_name: &str,
    arguments: Map<String, Value>,
) -> Value {
    client
        .call_tool(CallToolRequestParams::new(tool_name.to_owned()).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("tool {tool_name} should succeed: {error}"))
        .structured_content
        .expect("tool result should include structured content")
}

#[allow(dead_code)]
async fn read_resource_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: String,
) -> Value {
    let response = client
        .read_resource(rmcp::model::ReadResourceRequestParams::new(uri.clone()))
        .await
        .unwrap_or_else(|error| panic!("resource {uri} should be readable: {error}"));
    assert_eq!(response.contents.len(), 1);
    let text = match &response.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text,
        other => panic!("expected text resource contents, got {other:?}"),
    };
    serde_json::from_str(text).expect("resource contents should be valid JSON")
}

fn json_map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-operator-surface-direct-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-operator-surface-direct-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-operator-surface-direct-1",
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "us-election-2028",
                    "side": "yes",
                    "target_notional_usd": 3000
                },
                "constraints": {
                    "allowed_venues": ["polymarket", "kalshi"],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "normal",
                    "hedge_ratio_bps": 4000
                },
                "funding": {
                    "preferred_asset": "USDC",
                    "source_chain": "base"
                },
                "post_actions": [
                    {"action_type": "hedge", "venue": "hyperliquid"}
                ]
            },
            "rationale": {"summary": "bridge then enter and hedge", "main_risks": ["bridge delay"]},
            "execution_preferences": {"preview_only": false, "allow_fast_path": true}
        }),
    )
}
