mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use a2ex_daemon::{DaemonConfig, DaemonService, SignerHandoff};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOpenOrder, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::{PolicyDecision, PolicyEvaluator, PolicyInput};
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignedTx, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord, TxSignRequest,
};
use a2ex_state::StateRepository;
use a2ex_strategy_runtime::{RuntimeCommand, RuntimeWatcherState};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use support::hyperliquid_harness::FakeHyperliquidTransport;
use tempfile::tempdir;

const RUNTIME_CONTROL_TABLE: &str = "runtime_control";
const RUNTIME_CONTROL_SCOPE: &str = "autonomous_runtime";
const RUNTIME_CONTROL_COLUMNS: &[&str] = &[
    "scope_key",
    "control_mode",
    "transition_reason",
    "transition_source",
    "transitioned_at",
    "last_cleared_at",
    "last_cleared_reason",
    "last_cleared_source",
    "last_rejection_code",
    "last_rejection_message",
    "last_rejection_operation",
    "last_rejection_at",
    "updated_at",
];

#[derive(Default, Clone)]
struct PassiveSigner {
    handoffs: Arc<Mutex<Vec<String>>>,
}

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, request: &a2ex_daemon::ExecutionRequest) {
        self.handoffs
            .lock()
            .expect("handoff log lock")
            .push(request.action_kind.clone());
    }
}

#[derive(Default, Clone)]
struct SigningBridge {
    approvals: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl SignerBridge for SigningBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        self.approvals
            .lock()
            .expect("approval log lock")
            .push(req.action_kind.clone());
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }

    async fn sign_transaction(&self, req: TxSignRequest) -> Result<SignedTx, SignerBridgeError> {
        Ok(SignedTx { bytes: req.payload })
    }
}

#[derive(Clone, Default)]
struct AllowAllPolicy;

impl PolicyEvaluator for AllowAllPolicy {
    fn evaluate(&self, _input: &PolicyInput) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

#[derive(Debug, Clone)]
struct RuntimeControlRow {
    control_mode: String,
    transition_reason: String,
    transition_source: String,
    transitioned_at: String,
    last_cleared_at: Option<String>,
    last_cleared_reason: Option<String>,
    last_cleared_source: Option<String>,
    last_rejection_code: Option<String>,
    last_rejection_message: Option<String>,
    last_rejection_operation: Option<String>,
    last_rejection_at: Option<String>,
    updated_at: String,
}

#[tokio::test]
async fn runtime_stop_control_contract_requires_canonical_persistence_distinct_rejections_and_clear_recovery()
 {
    let mut gaps = Vec::new();

    let (
        _baseline_dir,
        baseline_config,
        baseline_service,
        _baseline_reservations,
        _baseline_signer,
        _baseline_bridge,
    ) = setup_stateful_service(seeded_harness()).await;
    register_strategy(&baseline_service).await;
    let manual_stop_commands = baseline_service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-stop".to_owned(),
                metric: "manual_stop".to_owned(),
                value: 1.0,
                cursor: "evt-stop".to_owned(),
                sampled_at: "2026-03-12T00:01:00Z".to_owned(),
            }],
            "2026-03-12T00:01:00Z",
        )
        .await
        .expect("manual_stop baseline should evaluate through the real runtime path");
    if !matches!(
        manual_stop_commands.first(),
        Some(RuntimeCommand::Unwind(_))
    ) {
        gaps.push(
            "manual_stop baseline must still emit an unwind command so stopped diagnostics can align with the existing runtime vocabulary"
                .to_owned(),
        );
    }
    let manual_stop_snapshot = StateRepository::open(baseline_config.state_db_path())
        .await
        .expect("baseline repo opens")
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("baseline snapshot loads")
        .expect("baseline snapshot exists after strategy registration");
    if manual_stop_snapshot.runtime_state != "unwinding" {
        gaps.push(format!(
            "manual_stop baseline must persist runtime_state=unwinding, found {}",
            manual_stop_snapshot.runtime_state
        ));
    }

    let (data_dir, config, service, reservations, signer, bridge) =
        setup_stateful_service(seeded_harness()).await;
    register_strategy(&service).await;

    reservations
        .hold(hold("reservation-prime", "rebalance-prime", 310))
        .await
        .expect("initial reservation persists");
    let rebalance_command = service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-1".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-12T00:00:10Z".to_owned(),
            }],
            "2026-03-12T00:00:10Z",
        )
        .await
        .expect("rebalance command evaluates")
        .into_iter()
        .next()
        .expect("real runtime flow should emit a rebalance command");
    service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command.clone(),
            "reservation-prime",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:00:11Z",
        )
        .await
        .expect("initial autonomous hedge should execute before runtime control is engaged");

    let state =
        Connection::open(config.state_db_path()).expect("state.db opens for control checks");
    let existing_columns = table_columns(&state, RUNTIME_CONTROL_TABLE);
    if existing_columns.is_empty() {
        gaps.push(
            "state.db is missing the canonical runtime_control table required for explicit stop/pause persistence"
                .to_owned(),
        );
    } else {
        require_columns(&existing_columns, RUNTIME_CONTROL_COLUMNS, &mut gaps);
    }
    ensure_contract_table_for_test_progression(&state);

    upsert_runtime_control(
        &state,
        RuntimeControlRow {
            control_mode: "paused".to_owned(),
            transition_reason: "operator_pause".to_owned(),
            transition_source: "direct_test".to_owned(),
            transitioned_at: "2026-03-12T00:02:00Z".to_owned(),
            last_cleared_at: None,
            last_cleared_reason: None,
            last_cleared_source: None,
            last_rejection_code: None,
            last_rejection_message: None,
            last_rejection_operation: None,
            last_rejection_at: None,
            updated_at: "2026-03-12T00:02:00Z".to_owned(),
        },
    );

    let paused_service =
        stateful_service_for_config(&config, seeded_harness(), signer.clone(), bridge.clone())
            .await;
    reservations
        .hold(hold("reservation-paused", "rebalance-paused", 310))
        .await
        .expect("paused reservation persists");
    let paused_result = paused_service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command.clone(),
            "reservation-paused",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:02:05Z",
        )
        .await;
    match paused_result {
        Ok(_) => gaps.push(
            "paused runtime control must reject new autonomous hedges before approvals/signing instead of allowing execution"
                .to_owned(),
        ),
        Err(error) => {
            let message = error.to_string();
            if !message.contains("runtime_paused") {
                gaps.push(format!(
                    "paused autonomous rejection must surface rejection_code=runtime_paused, got {message}"
                ));
            }
        }
    }
    let paused_row = load_runtime_control(&state);
    match paused_row.as_ref() {
        Some(row) => {
            if row.control_mode != "paused" {
                gaps.push(format!(
                    "paused runtime control must persist control_mode=paused, found {}",
                    row.control_mode
                ));
            }
            if row.last_rejection_code.as_deref() != Some("runtime_paused") {
                gaps.push(
                    "paused blocked action must persist last_rejection_code=runtime_paused"
                        .to_owned(),
                );
            }
            if row.last_rejection_operation.as_deref() != Some("strategy_rebalance") {
                gaps.push(
                    "paused blocked action must persist attempted_operation=strategy_rebalance"
                        .to_owned(),
                );
            }
            if row.last_rejection_at.is_none() {
                gaps.push(
                    "paused blocked action must persist last_rejection_at for restart-safe inspection"
                        .to_owned(),
                );
            }
        }
        None => gaps.push(
            "paused runtime control row should be readable from canonical state.db".to_owned(),
        ),
    }

    upsert_runtime_control(
        &state,
        RuntimeControlRow {
            control_mode: "stopped".to_owned(),
            transition_reason: "operator_stop".to_owned(),
            transition_source: "direct_test".to_owned(),
            transitioned_at: "2026-03-12T00:03:00Z".to_owned(),
            last_cleared_at: paused_row
                .as_ref()
                .and_then(|row| row.last_cleared_at.clone()),
            last_cleared_reason: paused_row
                .as_ref()
                .and_then(|row| row.last_cleared_reason.clone()),
            last_cleared_source: paused_row
                .as_ref()
                .and_then(|row| row.last_cleared_source.clone()),
            last_rejection_code: paused_row
                .as_ref()
                .and_then(|row| row.last_rejection_code.clone()),
            last_rejection_message: paused_row
                .as_ref()
                .and_then(|row| row.last_rejection_message.clone()),
            last_rejection_operation: paused_row
                .as_ref()
                .and_then(|row| row.last_rejection_operation.clone()),
            last_rejection_at: paused_row
                .as_ref()
                .and_then(|row| row.last_rejection_at.clone()),
            updated_at: "2026-03-12T00:03:00Z".to_owned(),
        },
    );

    let stopped_service =
        stateful_service_for_config(&config, seeded_harness(), signer.clone(), bridge.clone())
            .await;
    reservations
        .hold(hold("reservation-stopped", "rebalance-stopped", 310))
        .await
        .expect("stopped reservation persists");
    let stopped_result = stopped_service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command.clone(),
            "reservation-stopped",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:03:05Z",
        )
        .await;
    match stopped_result {
        Ok(_) => gaps.push(
            "stopped runtime control must reject new autonomous hedges before any new execution authority is exercised"
                .to_owned(),
        ),
        Err(error) => {
            let message = error.to_string();
            if !message.contains("runtime_stopped") {
                gaps.push(format!(
                    "stopped autonomous rejection must surface rejection_code=runtime_stopped, got {message}"
                ));
            }
            if !message.contains("manual_stop") {
                gaps.push(format!(
                    "stopped diagnostics must stay aligned with manual_stop-facing runtime wording, got {message}"
                ));
            }
        }
    }
    let stopped_row = load_runtime_control(&state);
    match stopped_row.as_ref() {
        Some(row) => {
            if row.control_mode != "stopped" {
                gaps.push(format!(
                    "stopped runtime control must persist control_mode=stopped, found {}",
                    row.control_mode
                ));
            }
            if row.last_rejection_code.as_deref() != Some("runtime_stopped") {
                gaps.push(
                    "stopped blocked action must persist last_rejection_code=runtime_stopped"
                        .to_owned(),
                );
            }
        }
        None => gaps.push(
            "stopped runtime control row should remain readable from canonical state.db".to_owned(),
        ),
    }

    let restarted_repo = StateRepository::open(config.state_db_path())
        .await
        .expect("restarted repo opens");
    let restarted_snapshot = restarted_repo
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("restarted snapshot loads")
        .expect("restarted snapshot exists");
    if restarted_snapshot.runtime_state != manual_stop_snapshot.runtime_state {
        gaps.push(format!(
            "stopped runtime inspection must stay aligned with manual_stop runtime_state={}, found {} after reread",
            manual_stop_snapshot.runtime_state, restarted_snapshot.runtime_state
        ));
    }

    let preserved_rejection_code = stopped_row
        .as_ref()
        .and_then(|row| row.last_rejection_code.clone());
    upsert_runtime_control(
        &state,
        RuntimeControlRow {
            control_mode: "active".to_owned(),
            transition_reason: "operator_clear_stop".to_owned(),
            transition_source: "direct_test".to_owned(),
            transitioned_at: "2026-03-12T00:04:00Z".to_owned(),
            last_cleared_at: Some("2026-03-12T00:04:00Z".to_owned()),
            last_cleared_reason: Some("operator_clear_stop".to_owned()),
            last_cleared_source: Some("direct_test".to_owned()),
            last_rejection_code: stopped_row
                .as_ref()
                .and_then(|row| row.last_rejection_code.clone()),
            last_rejection_message: stopped_row
                .as_ref()
                .and_then(|row| row.last_rejection_message.clone()),
            last_rejection_operation: stopped_row
                .as_ref()
                .and_then(|row| row.last_rejection_operation.clone()),
            last_rejection_at: stopped_row
                .as_ref()
                .and_then(|row| row.last_rejection_at.clone()),
            updated_at: "2026-03-12T00:04:00Z".to_owned(),
        },
    );

    let cleared_service =
        stateful_service_for_config(&config, seeded_harness(), signer, bridge).await;
    reservations
        .hold(hold("reservation-cleared", "rebalance-cleared", 310))
        .await
        .expect("cleared reservation persists");
    if let Err(error) = cleared_service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance_command,
            "reservation-cleared",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-12T00:04:05Z",
        )
        .await
    {
        gaps.push(format!(
            "clear-stop must explicitly restore autonomous eligibility, but execute_stateful_hedge still failed: {error}"
        ));
    }

    let cleared_row = load_runtime_control(&state);
    match cleared_row {
        Some(row) => {
            if row.control_mode != "active" {
                gaps.push(format!(
                    "clear-stop must persist control_mode=active, found {}",
                    row.control_mode
                ));
            }
            if row.last_cleared_at.is_none()
                || row.last_cleared_reason.as_deref() != Some("operator_clear_stop")
                || row.last_cleared_source.as_deref() != Some("direct_test")
            {
                gaps.push(
                    "clear-stop must persist last_cleared_at/last_cleared_reason/last_cleared_source metadata"
                        .to_owned(),
                );
            }
            if row.last_rejection_code != preserved_rejection_code {
                gaps.push(
                    "clear-stop must preserve the last blocked-action diagnostic instead of erasing it"
                        .to_owned(),
                );
            }
        }
        None => gaps.push(
            "clear-stop must keep the canonical runtime control row readable from state.db"
                .to_owned(),
        ),
    }

    assert!(
        gaps.is_empty(),
        "S03 direct runtime stop/pause contract missing canonical persistence or action gating: {}",
        gaps.join("; ")
    );

    drop(state);
    drop(data_dir);
}

async fn setup_stateful_service(
    harness: FakeHyperliquidTransport,
) -> (
    tempfile::TempDir,
    DaemonConfig,
    DaemonService<
        AllowAllPolicy,
        SqliteReservationManager,
        PassiveSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
        SimulatedEvmAdapter,
    >,
    SqliteReservationManager,
    PassiveSigner,
    SigningBridge,
) {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservations = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations");
    let signer = PassiveSigner::default();
    let bridge = SigningBridge::default();
    let service =
        stateful_service_for_config(&config, harness, signer.clone(), bridge.clone()).await;
    (data_dir, config, service, reservations, signer, bridge)
}

async fn stateful_service_for_config(
    config: &DaemonConfig,
    harness: FakeHyperliquidTransport,
    signer: PassiveSigner,
    bridge: SigningBridge,
) -> DaemonService<
    AllowAllPolicy,
    SqliteReservationManager,
    PassiveSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
    SimulatedEvmAdapter,
> {
    DaemonService::from_config_with_fast_path_and_hedge_adapter(
        config,
        AllowAllPolicy,
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations clone"),
        Arc::new(signer),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(bridge),
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

fn seeded_harness() -> FakeHyperliquidTransport {
    let harness = FakeHyperliquidTransport::default();
    seed_sync_state(
        &harness,
        "resting",
        "filled",
        "-0.5",
        "2026-03-12T00:00:11Z",
    );
    harness
}

fn seed_sync_state(
    harness: &FakeHyperliquidTransport,
    open_order_status: &str,
    order_status: &str,
    position_size: &str,
    filled_at: &str,
) {
    let open_orders = if open_order_status == "none" {
        Vec::new()
    } else {
        vec![HyperliquidOpenOrder {
            order_id: 91,
            asset: 7,
            instrument: "TOKEN-PERP".to_owned(),
            is_buy: false,
            price: "2412.7".to_owned(),
            size: "0.5".to_owned(),
            reduce_only: false,
            status: open_order_status.to_owned(),
            client_order_id: Some("hl-strategy-lp-1-1".to_owned()),
        }]
    };
    harness.seed_open_orders(open_orders);
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: order_status.to_owned(),
        filled_size: "0.5".to_owned(),
    });
    harness.seed_user_fills(vec![HyperliquidUserFill {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "0.5".to_owned(),
        price: "2412.7".to_owned(),
        side: "sell".to_owned(),
        filled_at: filled_at.to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: position_size.to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);
}

async fn register_strategy(
    service: &DaemonService<
        impl PolicyEvaluator,
        SqliteReservationManager,
        PassiveSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
        SimulatedEvmAdapter,
    >,
) {
    let response: JsonRpcResponse<a2ex_daemon::StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
}

fn hold(reservation_id: &str, execution_id: &str, amount: u64) -> ReservationRequest {
    ReservationRequest {
        reservation_id: reservation_id.to_owned(),
        execution_id: execution_id.to_owned(),
        asset: "USDC".to_owned(),
        amount,
    }
}

fn strategy_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-strategy-1",
        "daemon.registerStrategy",
        json!({
            "request_id": "req-strategy-1",
            "request_kind": "strategy",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
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

fn table_columns(connection: &Connection, table: &str) -> BTreeSet<String> {
    let mut statement = match connection.prepare(&format!("PRAGMA table_info({table})")) {
        Ok(statement) => statement,
        Err(_) => return BTreeSet::new(),
    };
    let rows = match statement.query_map([], |row| row.get::<_, String>(1)) {
        Ok(rows) => rows,
        Err(_) => return BTreeSet::new(),
    };
    rows.collect::<Result<BTreeSet<_>, _>>().unwrap_or_default()
}

fn require_columns(columns: &BTreeSet<String>, required: &[&str], gaps: &mut Vec<String>) {
    for column in required {
        if !columns.contains(*column) {
            gaps.push(format!(
                "runtime_control table is missing persisted column {column}"
            ));
        }
    }
}

fn ensure_contract_table_for_test_progression(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS runtime_control (
                scope_key TEXT PRIMARY KEY,
                control_mode TEXT NOT NULL,
                transition_reason TEXT NOT NULL,
                transition_source TEXT NOT NULL,
                transitioned_at TEXT NOT NULL,
                last_cleared_at TEXT,
                last_cleared_reason TEXT,
                last_cleared_source TEXT,
                last_rejection_code TEXT,
                last_rejection_message TEXT,
                last_rejection_operation TEXT,
                last_rejection_at TEXT,
                updated_at TEXT NOT NULL
            );",
        )
        .expect("runtime control contract table should be creatable for red-test progression");
}

fn upsert_runtime_control(connection: &Connection, row: RuntimeControlRow) {
    connection
        .execute(
            "INSERT INTO runtime_control (
                scope_key,
                control_mode,
                transition_reason,
                transition_source,
                transitioned_at,
                last_cleared_at,
                last_cleared_reason,
                last_cleared_source,
                last_rejection_code,
                last_rejection_message,
                last_rejection_operation,
                last_rejection_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(scope_key) DO UPDATE SET
                control_mode = excluded.control_mode,
                transition_reason = excluded.transition_reason,
                transition_source = excluded.transition_source,
                transitioned_at = excluded.transitioned_at,
                last_cleared_at = excluded.last_cleared_at,
                last_cleared_reason = excluded.last_cleared_reason,
                last_cleared_source = excluded.last_cleared_source,
                last_rejection_code = excluded.last_rejection_code,
                last_rejection_message = excluded.last_rejection_message,
                last_rejection_operation = excluded.last_rejection_operation,
                last_rejection_at = excluded.last_rejection_at,
                updated_at = excluded.updated_at",
            params![
                RUNTIME_CONTROL_SCOPE,
                row.control_mode,
                row.transition_reason,
                row.transition_source,
                row.transitioned_at,
                row.last_cleared_at,
                row.last_cleared_reason,
                row.last_cleared_source,
                row.last_rejection_code,
                row.last_rejection_message,
                row.last_rejection_operation,
                row.last_rejection_at,
                row.updated_at,
            ],
        )
        .expect("runtime control contract row upserts");
}

fn load_runtime_control(connection: &Connection) -> Option<RuntimeControlRow> {
    connection
        .query_row(
            "SELECT control_mode, transition_reason, transition_source, transitioned_at,
                    last_cleared_at, last_cleared_reason, last_cleared_source,
                    last_rejection_code, last_rejection_message, last_rejection_operation,
                    last_rejection_at, updated_at
             FROM runtime_control WHERE scope_key = ?1",
            [RUNTIME_CONTROL_SCOPE],
            |row| {
                Ok(RuntimeControlRow {
                    control_mode: row.get(0)?,
                    transition_reason: row.get(1)?,
                    transition_source: row.get(2)?,
                    transitioned_at: row.get(3)?,
                    last_cleared_at: row.get(4)?,
                    last_cleared_reason: row.get(5)?,
                    last_cleared_source: row.get(6)?,
                    last_rejection_code: row.get(7)?,
                    last_rejection_message: row.get(8)?,
                    last_rejection_operation: row.get(9)?,
                    last_rejection_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            },
        )
        .optional()
        .expect("runtime control row should be queryable")
}
