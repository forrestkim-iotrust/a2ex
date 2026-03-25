use std::sync::Arc;

use a2ex_daemon::{
    AuthorizationResult, BootstrapSource, CanonicalStateSnapshot, DaemonConfig, DaemonService,
    ExecutionRequest, ExecutionStateRecord, JournalEntry, ReconciliationStateRecord, SignerHandoff,
    StrategyRuntimeStateRecord, load_event_journal, load_runtime_state, persist_canonical_state,
    spawn_local_daemon,
};
use a2ex_ipc::{DAEMON_CONTROL_METHOD, JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord,
};
use rusqlite::Connection;
use tempfile::tempdir;
use tokio::fs;

#[derive(Debug, Default)]
struct RecordingBridge;

#[derive(Debug, Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &ExecutionRequest) {}
}

#[async_trait::async_trait]
impl SignerBridge for RecordingBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }
}

#[tokio::test]
async fn daemon_restarts_without_remote() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());

    let first = spawn_local_daemon(config.clone())
        .await
        .expect("first boot succeeds");
    let first_bootstrap = first.bootstrap().clone();
    first.shutdown().await.expect("first shutdown succeeds");

    let second = spawn_local_daemon(config)
        .await
        .expect("second boot succeeds");
    let second_bootstrap = second.bootstrap().clone();
    second.shutdown().await.expect("second shutdown succeeds");

    assert_eq!(first_bootstrap.source, BootstrapSource::LocalRuntime);
    assert_eq!(second_bootstrap.source, BootstrapSource::LocalRuntime);
    assert!(!first_bootstrap.used_remote_control_plane);
    assert!(!second_bootstrap.used_remote_control_plane);
    assert_eq!(
        first_bootstrap.state_db_path,
        second_bootstrap.state_db_path
    );
    assert_eq!(
        first_bootstrap.analytics_db_path,
        second_bootstrap.analytics_db_path
    );
}

#[tokio::test]
async fn restart_reuses_local_bootstrap_path() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());

    let first = spawn_local_daemon(config.clone())
        .await
        .expect("first boot succeeds");
    let first_bootstrap = first.bootstrap().clone();
    first.shutdown().await.expect("first shutdown succeeds");

    let second = spawn_local_daemon(config)
        .await
        .expect("second boot succeeds");
    let second_bootstrap = second.bootstrap().clone();
    second.shutdown().await.expect("second shutdown succeeds");

    assert_eq!(first_bootstrap.bootstrap_path, "local-runtime");
    assert_eq!(second_bootstrap.bootstrap_path, "local-runtime");
    assert_eq!(
        first_bootstrap.bootstrap_path,
        second_bootstrap.bootstrap_path
    );
    assert!(second_bootstrap.recovered_existing_state);
}

#[tokio::test]
async fn canonical_state_persists_to_state_db() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let snapshot = CanonicalStateSnapshot {
        strategies: vec![StrategyRuntimeStateRecord {
            strategy_id: "strategy-1".to_owned(),
            runtime_state: "active".to_owned(),
            last_transition_at: "2026-03-10T00:00:00Z".to_owned(),
            updated_at: "2026-03-10T00:00:00Z".to_owned(),
        }],
        executions: vec![ExecutionStateRecord {
            execution_id: "execution-1".to_owned(),
            plan_id: "plan-1".to_owned(),
            status: "executing".to_owned(),
            updated_at: "2026-03-10T00:00:01Z".to_owned(),
        }],
        reconciliations: vec![ReconciliationStateRecord {
            execution_id: "execution-1".to_owned(),
            residual_exposure_usd: 135,
            rebalance_required: true,
            updated_at: "2026-03-10T00:00:02Z".to_owned(),
        }],
    };

    persist_canonical_state(&config, &snapshot)
        .await
        .expect("canonical state persists");

    let recovered = load_runtime_state(&config)
        .await
        .expect("runtime state reloads from sqlite");
    let journal = load_event_journal(&config)
        .await
        .expect("journal entries reload from sqlite");

    assert_eq!(recovered, snapshot);
    assert_eq!(journal.len(), 3);
    assert_eq!(
        journal
            .iter()
            .map(JournalEntry::event_type)
            .collect::<Vec<_>>(),
        vec![
            "strategy_state_changed",
            "execution_state_changed",
            "reconciliation_state_changed",
        ]
    );
}

#[tokio::test]
async fn daemon_restart_reloads_canonical_state_from_state_db() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let snapshot = CanonicalStateSnapshot {
        strategies: vec![StrategyRuntimeStateRecord {
            strategy_id: "strategy-7".to_owned(),
            runtime_state: "rebalancing".to_owned(),
            last_transition_at: "2026-03-10T00:01:00Z".to_owned(),
            updated_at: "2026-03-10T00:01:00Z".to_owned(),
        }],
        executions: vec![ExecutionStateRecord {
            execution_id: "execution-7".to_owned(),
            plan_id: "plan-7".to_owned(),
            status: "reconciling".to_owned(),
            updated_at: "2026-03-10T00:01:01Z".to_owned(),
        }],
        reconciliations: vec![ReconciliationStateRecord {
            execution_id: "execution-7".to_owned(),
            residual_exposure_usd: 42,
            rebalance_required: false,
            updated_at: "2026-03-10T00:01:02Z".to_owned(),
        }],
    };

    persist_canonical_state(&config, &snapshot)
        .await
        .expect("canonical state persists before restart");

    let first = spawn_local_daemon(config.clone())
        .await
        .expect("first boot succeeds");
    first.shutdown().await.expect("first shutdown succeeds");

    let second = spawn_local_daemon(config.clone())
        .await
        .expect("second boot succeeds");
    let second_bootstrap = second.bootstrap().clone();
    second.shutdown().await.expect("second shutdown succeeds");

    let recovered = load_runtime_state(&config)
        .await
        .expect("daemon reloads canonical state after restart");

    assert!(!second_bootstrap.used_remote_control_plane);
    assert!(second_bootstrap.recovered_existing_state);
    assert_eq!(recovered, snapshot);
}

#[tokio::test]
async fn canonical_execution_commit_happens_after_signer_dispatch() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-commit".to_owned(),
            execution_id: "exec-commit".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let bridge = Arc::new(RecordingBridge);
    let signer = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        bridge,
        LocalPeerValidator::strict_local_only(),
    );
    let service = DaemonService::from_config_with_signer_bridge(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        signer,
    );

    let response: JsonRpcResponse<AuthorizationResult> = service
        .authorize_and_dispatch_to_signer(
            JsonRpcRequest::new(
                "req-canonical-commit",
                DAEMON_CONTROL_METHOD,
                ExecutionRequest {
                    action_id: "exec-commit".to_owned(),
                    action_kind: "submit_intent".to_owned(),
                    notional_usd: 25,
                    reservation_id: "reservation-commit".to_owned(),
                },
            ),
            LocalPeerIdentity::for_tests(true, true),
        )
        .await
        .expect("response builds");

    assert!(matches!(response, JsonRpcResponse::Success(_)));

    let recovered = load_runtime_state(&config)
        .await
        .expect("runtime state reloads from sqlite after dispatch");

    assert_eq!(recovered.executions.len(), 1);
    assert_eq!(recovered.executions[0].execution_id, "exec-commit");
    assert_eq!(recovered.executions[0].plan_id, "submit_intent");
    assert_eq!(recovered.executions[0].status, "dispatched");
    assert!(!recovered.executions[0].updated_at.is_empty());

    let journal = load_event_journal(&config)
        .await
        .expect("journal reloads after dispatch");
    assert!(journal.iter().any(|entry| {
        entry.stream_id == "exec-commit" && entry.event_type() == "execution_state_changed"
    }));
}

#[tokio::test]
async fn restart_reloads_canonical_execution_committed_after_dispatch() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-restart".to_owned(),
            execution_id: "exec-restart".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let bridge = Arc::new(RecordingBridge);
    let signer = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        bridge,
        LocalPeerValidator::strict_local_only(),
    );
    let service = DaemonService::from_config_with_signer_bridge(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        signer,
    );

    let response: JsonRpcResponse<AuthorizationResult> = service
        .authorize_and_dispatch_to_signer(
            JsonRpcRequest::new(
                "req-restart-commit",
                DAEMON_CONTROL_METHOD,
                ExecutionRequest {
                    action_id: "exec-restart".to_owned(),
                    action_kind: "submit_intent".to_owned(),
                    notional_usd: 25,
                    reservation_id: "reservation-restart".to_owned(),
                },
            ),
            LocalPeerIdentity::for_tests(true, true),
        )
        .await
        .expect("response builds");

    assert!(matches!(response, JsonRpcResponse::Success(_)));

    let daemon = spawn_local_daemon(config.clone())
        .await
        .expect("daemon boot succeeds");
    daemon.shutdown().await.expect("daemon shutdown succeeds");

    let recovered = load_runtime_state(&config)
        .await
        .expect("runtime state reloads after restart");

    assert_eq!(recovered.executions.len(), 1);
    assert_eq!(recovered.executions[0].execution_id, "exec-restart");
    assert_eq!(recovered.executions[0].status, "dispatched");
}

#[tokio::test]
async fn split_state_and_analytics_dbs() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-analytics".to_owned(),
            execution_id: "exec-analytics".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let bridge = Arc::new(RecordingBridge);
    let signer = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        bridge,
        LocalPeerValidator::strict_local_only(),
    );
    let service = DaemonService::from_config_with_signer_bridge(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        signer,
    );

    let response: JsonRpcResponse<AuthorizationResult> = service
        .authorize_and_dispatch_to_signer(
            JsonRpcRequest::new(
                "req-analytics",
                DAEMON_CONTROL_METHOD,
                ExecutionRequest {
                    action_id: "exec-analytics".to_owned(),
                    action_kind: "submit_intent".to_owned(),
                    notional_usd: 25,
                    reservation_id: "reservation-analytics".to_owned(),
                },
            ),
            LocalPeerIdentity::for_tests(true, true),
        )
        .await
        .expect("response builds");

    assert!(matches!(response, JsonRpcResponse::Success(_)));

    let state = load_runtime_state(&config)
        .await
        .expect("state db remains canonical");
    assert_eq!(state.executions.len(), 1);

    let analytics = Connection::open(config.analytics_db_path()).expect("analytics db opens");
    let projection: (String, String) = analytics
        .query_row(
            "SELECT execution_id, status FROM execution_projection WHERE execution_id = ?1",
            ["exec-analytics"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("analytics projection row exists");

    assert_eq!(projection.0, "exec-analytics");
    assert_eq!(projection.1, "dispatched");
}

#[tokio::test]
async fn canonical_writes_survive_analytics_projection_unavailable() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    fs::create_dir_all(config.analytics_db_path())
        .await
        .expect("analytics path blocked by directory");

    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: "reservation-analytics-down".to_owned(),
            execution_id: "exec-analytics-down".to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let bridge = Arc::new(RecordingBridge);
    let signer = a2ex_signer_bridge::LocalSignerBridgeClient::new(
        bridge,
        LocalPeerValidator::strict_local_only(),
    );
    let service = DaemonService::from_config_with_signer_bridge(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        signer,
    );

    let response: JsonRpcResponse<AuthorizationResult> = service
        .authorize_and_dispatch_to_signer(
            JsonRpcRequest::new(
                "req-analytics-down",
                DAEMON_CONTROL_METHOD,
                ExecutionRequest {
                    action_id: "exec-analytics-down".to_owned(),
                    action_kind: "submit_intent".to_owned(),
                    notional_usd: 25,
                    reservation_id: "reservation-analytics-down".to_owned(),
                },
            ),
            LocalPeerIdentity::for_tests(true, true),
        )
        .await
        .expect("response builds");

    assert!(matches!(response, JsonRpcResponse::Success(_)));

    let state = load_runtime_state(&config)
        .await
        .expect("canonical commit succeeds without analytics freshness");
    assert_eq!(state.executions.len(), 1);
    assert_eq!(state.executions[0].execution_id, "exec-analytics-down");
}
