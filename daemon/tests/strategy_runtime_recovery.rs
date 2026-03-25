mod support;

use std::sync::Arc;
use std::time::Duration;

use a2ex_daemon::{
    DaemonConfig, DaemonService, SignerHandoff, StrategyRegistrationReceipt, spawn_local_daemon,
    spawn_local_daemon_with_service,
};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome};
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidOrderStatus, HyperliquidPosition, HyperliquidUserFill,
};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignedTx, SignerBridge, SignerBridgeError,
    SignerBridgeRequestRecord, TxSignRequest,
};
use a2ex_state::{PersistedPendingHedge, PersistedTriggerMemory, StateRepository};
use a2ex_strategy_runtime::RuntimeWatcherState;
use async_trait::async_trait;
use serde_json::json;
use support::hyperliquid_harness::FakeHyperliquidTransport;
use tempfile::tempdir;
use tokio::time::sleep;

#[derive(Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {}
}

#[derive(Default, Clone)]
struct SigningBridge;

#[async_trait]
impl SignerBridge for SigningBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }

    async fn sign_transaction(&self, req: TxSignRequest) -> Result<SignedTx, SignerBridgeError> {
        Ok(SignedTx { bytes: req.payload })
    }
}

#[tokio::test]
async fn strategy_runtime_restores_idle_strategies_after_restart() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservations = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations");
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservations,
        Arc::new(PassiveSigner),
    );
    register_strategy(&service).await;

    let daemon = spawn_local_daemon(config.clone())
        .await
        .expect("daemon boots");
    daemon.shutdown().await.expect("daemon shuts down");

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("exists");
    assert_eq!(snapshot.runtime_state, "idle");
}

#[tokio::test]
async fn strategy_runtime_bootstrap_recovery_preserves_cooldown_without_manual_helpers() {
    let harness = FakeHyperliquidTransport::default();
    harness.seed_open_orders(Vec::new());
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
        filled_at: "2026-03-11T00:00:20Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "-0.5".to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);

    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservations = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations");
    let service = DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        BaselinePolicy::new(100),
        reservations,
        Arc::new(PassiveSigner),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 10,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    );
    register_strategy(&service).await;

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let mut snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("exists");
    snapshot.runtime_state = "rebalancing".to_owned();
    snapshot.trigger_memory = vec![PersistedTriggerMemory {
        trigger_key: "trigger-0".to_owned(),
        cooldown_until: Some("2026-03-11T00:00:30Z".to_owned()),
        last_fired_at: Some("2026-03-11T00:00:20Z".to_owned()),
        hysteresis_armed: true,
    }];
    snapshot.pending_hedge = Some(PersistedPendingHedge {
        venue: "hyperliquid".to_owned(),
        instrument: "TOKEN-PERP".to_owned(),
        client_order_id: "hl-strategy-lp-1-1".to_owned(),
        signer_address: "hl-signer-strategy-lp-1".to_owned(),
        account_address: "hl-account-strategy-lp-1".to_owned(),
        order_id: Some(91),
        nonce: 1,
        status: "submitted".to_owned(),
        last_synced_at: None,
    });
    snapshot.updated_at = "2026-03-11T00:00:20Z".to_owned();
    repository
        .persist_strategy_recovery_snapshot(&snapshot)
        .await
        .expect("snapshot persists");

    let daemon = spawn_local_daemon_with_service(config.clone(), service)
        .await
        .expect("daemon boots with recovery supervisors");

    assert_eq!(
        daemon.active_runtime_supervisors(),
        vec!["strategy-lp-1".to_owned()]
    );

    sleep(Duration::from_millis(120)).await;

    let recovering = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot reloads")
        .expect("exists");
    assert_eq!(recovering.runtime_state, "recovering");
    assert_eq!(recovering.metrics["warm"], true);
    assert_eq!(recovering.metrics["venue_sync_required"], true);
    assert!(daemon.take_runtime_commands("strategy-lp-1").is_empty());

    daemon
        .publish_runtime_watcher_sample(
            "strategy-lp-1",
            RuntimeWatcherState {
                watcher_key: "lp-position".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.031,
                cursor: "evt-warm".to_owned(),
                sampled_at: "2026-03-11T00:00:25Z".to_owned(),
            },
        )
        .await
        .expect("warm watcher sample publishes");

    sleep(Duration::from_millis(150)).await;

    let recovered = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot reloads")
        .expect("exists");
    assert!(daemon.take_runtime_commands("strategy-lp-1").is_empty());
    assert_eq!(recovered.runtime_state, "active");
    assert_eq!(recovered.metrics["warm"], false);
    assert_eq!(recovered.metrics["venue_sync_required"], false);
    assert_eq!(
        recovered.trigger_memory[0].cooldown_until.as_deref(),
        Some("2026-03-11T00:00:30Z")
    );
    assert_eq!(
        recovered.pending_hedge.expect("pending hedge").status,
        "filled"
    );
    assert_eq!(harness.info_requests().len(), 4);

    daemon.shutdown().await.expect("daemon shuts down");
}

#[tokio::test]
async fn strategy_runtime_recovers_active_hedges_after_restart() {
    let harness = FakeHyperliquidTransport::default();
    harness.seed_open_orders(Vec::new());
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
        filled_at: "2026-03-11T00:00:20Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "-0.5".to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);

    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservations = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservations");
    let service = DaemonService::from_config_with_fast_path_and_hedge_adapter(
        &config,
        BaselinePolicy::new(100),
        reservations,
        Arc::new(PassiveSigner),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 10,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
        HyperliquidAdapter::with_transport(harness.transport(), 0),
    );
    register_strategy(&service).await;

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("repo opens");
    let mut snapshot = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot loads")
        .expect("exists");
    snapshot.runtime_state = "rebalancing".to_owned();
    snapshot.pending_hedge = Some(PersistedPendingHedge {
        venue: "hyperliquid".to_owned(),
        instrument: "TOKEN-PERP".to_owned(),
        client_order_id: "hl-strategy-lp-1-1".to_owned(),
        signer_address: "hl-signer-strategy-lp-1".to_owned(),
        account_address: "hl-account-strategy-lp-1".to_owned(),
        order_id: Some(91),
        nonce: 1,
        status: "submitted".to_owned(),
        last_synced_at: None,
    });
    snapshot.updated_at = "2026-03-11T00:00:10Z".to_owned();
    repository
        .persist_strategy_recovery_snapshot(&snapshot)
        .await
        .expect("snapshot persists");

    let daemon = spawn_local_daemon_with_service(config.clone(), service)
        .await
        .expect("daemon boots with recovery supervisors");

    daemon
        .publish_runtime_watcher_sample(
            "strategy-lp-1",
            RuntimeWatcherState {
                watcher_key: "lp-position".to_owned(),
                metric: "delta_exposure_pct".to_owned(),
                value: 0.01,
                cursor: "evt-warm".to_owned(),
                sampled_at: "2026-03-11T00:00:20Z".to_owned(),
            },
        )
        .await
        .expect("warm watcher sample publishes");

    sleep(Duration::from_millis(150)).await;

    let recovered = repository
        .load_strategy_recovery_snapshot("strategy-lp-1")
        .await
        .expect("snapshot reloads")
        .expect("exists");
    assert!(daemon.take_runtime_commands("strategy-lp-1").is_empty());
    assert_eq!(recovered.runtime_state, "active");
    assert_eq!(recovered.metrics["warm"], false);
    assert_eq!(recovered.metrics["venue_sync_required"], false);
    assert_eq!(
        recovered.pending_hedge.expect("pending hedge").status,
        "filled"
    );
    assert_eq!(harness.info_requests().len(), 4);

    daemon.shutdown().await.expect("daemon shuts down");
}

async fn register_strategy(
    service: &DaemonService<
        impl a2ex_policy::PolicyEvaluator,
        SqliteReservationManager,
        PassiveSigner,
        impl a2ex_signer_bridge::ValidatedSignerBridge,
        impl a2ex_evm_adapter::EvmAdapter,
    >,
) {
    let response: JsonRpcResponse<StrategyRegistrationReceipt> = service
        .register_strategy(strategy_request())
        .await
        .expect("register strategy");
    assert!(matches!(response, JsonRpcResponse::Success(_)));
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
