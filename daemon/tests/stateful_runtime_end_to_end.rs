mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2ex_daemon::{
    DaemonConfig, DaemonService, SignerHandoff, StrategyRegistrationReceipt,
    spawn_local_daemon_with_service,
};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome, TxLifecycleStatus};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidExchangeRequest, HyperliquidInfoRequest, HyperliquidOpenOrder,
    HyperliquidOrderStatus, HyperliquidPosition, HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::{BaselinePolicy, PolicyDecision, PolicyEvaluator, PolicyInput};
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignedTx, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord, TxSignRequest,
};
use a2ex_state::StateRepository;
use a2ex_strategy_runtime::RuntimeWatcherState;
use async_trait::async_trait;
use serde_json::json;
use support::hyperliquid_harness::FakeHyperliquidTransport;
use tempfile::tempdir;
use tokio::time::sleep;

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
    sign_payloads: Arc<Mutex<Vec<Vec<u8>>>>,
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
        self.sign_payloads
            .lock()
            .expect("sign payload lock")
            .push(req.payload.clone());
        Ok(SignedTx { bytes: req.payload })
    }
}

#[derive(Clone)]
struct RecordingPolicy {
    blocked_action_kind: Option<String>,
    evaluations: Arc<Mutex<Vec<String>>>,
}

impl RecordingPolicy {
    fn allowing() -> Self {
        Self {
            blocked_action_kind: None,
            evaluations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn blocking(action_kind: &str) -> Self {
        Self {
            blocked_action_kind: Some(action_kind.to_owned()),
            evaluations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn evaluations(&self) -> Vec<String> {
        self.evaluations
            .lock()
            .expect("policy evaluation lock")
            .clone()
    }
}

impl PolicyEvaluator for RecordingPolicy {
    fn evaluate(&self, input: &PolicyInput) -> PolicyDecision {
        self.evaluations
            .lock()
            .expect("policy evaluation lock")
            .push(input.action_kind.clone());
        if self
            .blocked_action_kind
            .as_ref()
            .is_some_and(|blocked| blocked == &input.action_kind)
        {
            return PolicyDecision::Hold {
                reason: format!("{} requires manual review", input.action_kind),
            };
        }
        PolicyDecision::Allow
    }
}

#[tokio::test]
async fn strategy_runtime_dispatches_rebalance_and_unwind_intents() {
    let (_data_dir, config, service, reservations) = setup_service().await;
    register_strategy(&service).await;
    reservations
        .hold(hold("reservation-rebalance", "rebalance", 310))
        .await
        .expect("hold persists");

    let commands = service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-1".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-11T00:00:10Z".to_owned(),
            }],
            "2026-03-11T00:00:10Z",
        )
        .await
        .expect("evaluate strategy");
    let report = service
        .execute_stateful_hedge(
            "strategy-lp-1",
            commands[0].clone(),
            "reservation-rebalance",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-11T00:00:11Z",
        )
        .await
        .expect("rebalance executes");
    assert_eq!(
        report.terminal_status(),
        Some(&TxLifecycleStatus::Confirmed)
    );

    reservations
        .hold(hold("reservation-unwind", "unwind", 1_000))
        .await
        .expect("hold persists");
    let unwind_commands = service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-stop".to_owned(),
                metric: "manual_stop".to_owned(),
                value: 1.0,
                cursor: "evt-2".to_owned(),
                sampled_at: "2026-03-11T00:01:00Z".to_owned(),
            }],
            "2026-03-11T00:01:00Z",
        )
        .await
        .expect("evaluate unwind");
    let unwind = service
        .execute_stateful_hedge(
            "strategy-lp-1",
            unwind_commands[0].clone(),
            "reservation-unwind",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-11T00:01:01Z",
        )
        .await
        .expect("unwind executes");
    assert_eq!(
        unwind.terminal_status(),
        Some(&TxLifecycleStatus::Confirmed)
    );

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("exists");
    assert_eq!(snapshot.pending_hedge.expect("hedge").nonce, 2);
}

#[tokio::test]
async fn stateful_strategy_runtime_end_to_end_handles_rebalance_unwind_and_restart() {
    let harness = FakeHyperliquidTransport::default();
    seed_sync_state(
        &harness,
        "resting",
        "filled",
        "-0.5",
        "2026-03-11T00:00:11Z",
    );
    let (_data_dir, config, service, reservations, policy, signer, bridge) =
        setup_stateful_service(RecordingPolicy::allowing(), harness.clone()).await;
    register_strategy(&service).await;

    reservations
        .hold(hold("reservation-rebalance", "rebalance", 310))
        .await
        .expect("hold persists");
    let rebalance = service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-1".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-11T00:00:10Z".to_owned(),
            }],
            "2026-03-11T00:00:10Z",
        )
        .await
        .expect("evaluate strategy");
    let report = service
        .execute_stateful_hedge(
            "strategy-lp-1",
            rebalance[0].clone(),
            "reservation-rebalance",
            LocalPeerIdentity::for_tests(true, true),
            "2026-03-11T00:00:11Z",
        )
        .await
        .expect("rebalance executes");
    assert_eq!(
        report.terminal_status(),
        Some(&TxLifecycleStatus::Confirmed)
    );

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let post_rebalance = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("exists");
    let rebalance_hedge = post_rebalance.pending_hedge.expect("pending hedge");
    let (expected_signer, expected_account) = expected_runtime_identities("strategy-lp-1");
    assert_eq!(policy.evaluations(), vec!["strategy_rebalance"]);
    assert_eq!(
        bridge.approvals.lock().expect("approval lock").as_slice(),
        ["strategy_rebalance"]
    );
    assert_eq!(
        signer.handoffs.lock().expect("handoff lock").as_slice(),
        ["strategy_rebalance"]
    );
    assert_eq!(rebalance_hedge.status, "filled");
    assert_eq!(
        rebalance_hedge.last_synced_at.as_deref(),
        Some("2026-03-11T00:00:11Z")
    );
    assert_eq!(post_rebalance.runtime_state, "active");
    assert_eq!(rebalance_hedge.signer_address, expected_signer);
    assert_eq!(rebalance_hedge.account_address, expected_account);
    assert!(matches!(
        harness.exchange_requests().first(),
        Some(HyperliquidExchangeRequest::Place(request))
            if request.signer_address == rebalance_hedge.signer_address
                && request.account_address == rebalance_hedge.account_address
    ));

    let blocked_policy = RecordingPolicy::blocking("strategy_rebalance");
    let (
        _other_dir,
        _other_config,
        blocked_service,
        blocked_reservations,
        blocked_recorder,
        blocked_signer,
        blocked_bridge,
    ) = setup_stateful_service(blocked_policy.clone(), FakeHyperliquidTransport::default()).await;
    register_strategy(&blocked_service).await;
    blocked_reservations
        .hold(hold("blocked-reservation", "rebalance", 310))
        .await
        .expect("blocked hold persists");
    let blocked_commands = blocked_service
        .evaluate_strategy(
            "strategy-lp-1",
            vec![RuntimeWatcherState {
                watcher_key: "w-1".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-1".to_owned(),
                sampled_at: "2026-03-11T00:00:10Z".to_owned(),
            }],
            "2026-03-11T00:00:10Z",
        )
        .await
        .expect("blocked strategy evaluates");
    assert!(
        blocked_service
            .execute_stateful_hedge(
                "strategy-lp-1",
                blocked_commands[0].clone(),
                "blocked-reservation",
                LocalPeerIdentity::for_tests(true, true),
                "2026-03-11T00:00:11Z",
            )
            .await
            .is_err()
    );
    assert_eq!(blocked_recorder.evaluations(), vec!["strategy_rebalance"]);
    assert!(
        blocked_bridge
            .approvals
            .lock()
            .expect("blocked approval lock")
            .is_empty()
    );
    assert!(
        blocked_signer
            .handoffs
            .lock()
            .expect("blocked handoff lock")
            .is_empty()
    );

    let restart_service = stateful_service_for_config(
        &config,
        RecordingPolicy::allowing(),
        harness.clone(),
        PassiveSigner::default(),
        SigningBridge::default(),
    )
    .await;
    let restart = spawn_local_daemon_with_service(config.clone(), restart_service)
        .await
        .expect("daemon boots");

    let recovered = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("recovered snapshot loads")
        .expect("exists");
    assert_eq!(recovered.runtime_state, "recovering");

    seed_sync_state(&harness, "none", "filled", "-0.5", "2026-03-11T00:00:20Z");
    restart
        .publish_runtime_watcher_sample(
            "strategy-lp-1",
            RuntimeWatcherState {
                watcher_key: "w-warm".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.0,
                cursor: "evt-warm".to_owned(),
                sampled_at: "2026-03-11T00:00:20Z".to_owned(),
            },
        )
        .await
        .expect("warm watcher sample publishes");
    sleep(Duration::from_millis(150)).await;
    let _ = restart.take_runtime_commands("strategy-lp-1");

    restart
        .publish_runtime_watcher_sample(
            "strategy-lp-1",
            RuntimeWatcherState {
                watcher_key: "w-stop".to_owned(),
                metric: "manual_stop".to_owned(),
                value: 1.0,
                cursor: "evt-2".to_owned(),
                sampled_at: "2026-03-11T00:01:00Z".to_owned(),
            },
        )
        .await
        .expect("manual stop sample publishes");
    sleep(Duration::from_millis(150)).await;
    let unwind = restart
        .take_runtime_commands("strategy-lp-1")
        .into_iter()
        .find(|command| matches!(command, a2ex_strategy_runtime::RuntimeCommand::Unwind(_)))
        .expect("daemon runtime emits unwind command");
    let snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("exists");
    let hedge = snapshot.pending_hedge.expect("pending hedge");
    let journal = repository.load_journal().await.expect("journal loads");
    let info_requests = harness.info_requests();

    assert!(matches!(
        unwind,
        a2ex_strategy_runtime::RuntimeCommand::Unwind(_)
    ));
    assert_eq!(snapshot.runtime_state, "unwinding");
    assert_eq!(snapshot.metrics["venue_sync_required"], false);
    assert_eq!(hedge.nonce, 1);
    assert_eq!(hedge.status, "filled");
    assert_eq!(hedge.signer_address, expected_signer);
    assert_eq!(hedge.account_address, expected_account);
    assert!(info_requests.iter().all(|request| match request {
        HyperliquidInfoRequest::OpenOrders { account_address }
        | HyperliquidInfoRequest::UserFills {
            account_address, ..
        }
        | HyperliquidInfoRequest::ClearinghouseState { account_address }
        | HyperliquidInfoRequest::OrderStatus {
            account_address, ..
        } => account_address == &hedge.account_address,
    }));
    assert!(info_requests.iter().all(|request| match request {
        HyperliquidInfoRequest::OpenOrders { account_address }
        | HyperliquidInfoRequest::UserFills {
            account_address, ..
        }
        | HyperliquidInfoRequest::ClearinghouseState { account_address }
        | HyperliquidInfoRequest::OrderStatus {
            account_address, ..
        } => account_address != "local-account",
    }));
    assert!(journal.iter().any(|entry| {
        entry.event_type == "execution_state_changed"
            && entry.stream_id.starts_with("hl-strategy-lp-1-")
            && entry.payload_json.contains("filled")
    }));

    restart.shutdown().await.expect("daemon shuts down");
}

fn expected_runtime_identities(strategy_id: &str) -> (String, String) {
    (
        format!("hl-signer-{strategy_id}"),
        format!("hl-account-{strategy_id}"),
    )
}

async fn setup_service() -> (
    tempfile::TempDir,
    DaemonConfig,
    DaemonService<
        BaselinePolicy,
        SqliteReservationManager,
        PassiveSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
        SimulatedEvmAdapter,
    >,
    SqliteReservationManager,
) {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let harness = FakeHyperliquidTransport::default();
    seed_sync_state(
        &harness,
        "resting",
        "filled",
        "-0.5",
        "2026-03-11T00:00:11Z",
    );
    let reservations = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations");
    let service = DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        BaselinePolicy::new(1_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations clone"),
        Arc::new(PassiveSigner::default()),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 10,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    );
    (data_dir, config, service, reservations)
}

async fn setup_stateful_service(
    policy: RecordingPolicy,
    harness: FakeHyperliquidTransport,
) -> (
    tempfile::TempDir,
    DaemonConfig,
    DaemonService<
        RecordingPolicy,
        SqliteReservationManager,
        PassiveSigner,
        a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
        SimulatedEvmAdapter,
    >,
    SqliteReservationManager,
    RecordingPolicy,
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
    let service = stateful_service_for_config(
        &config,
        policy.clone(),
        harness,
        signer.clone(),
        bridge.clone(),
    )
    .await;
    (
        data_dir,
        config,
        service,
        reservations,
        policy,
        signer,
        bridge,
    )
}

async fn stateful_service_for_config(
    config: &DaemonConfig,
    policy: RecordingPolicy,
    harness: FakeHyperliquidTransport,
    signer: PassiveSigner,
    bridge: SigningBridge,
) -> DaemonService<
    RecordingPolicy,
    SqliteReservationManager,
    PassiveSigner,
    a2ex_signer_bridge::LocalSignerBridgeClient<SigningBridge>,
    SimulatedEvmAdapter,
> {
    DaemonService::from_config_with_fast_path_and_hedge_adapter(
        config,
        policy,
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
    let response: JsonRpcResponse<StrategyRegistrationReceipt> = service
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
            "submitted_at": "2026-03-11T00:00:00Z",
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
