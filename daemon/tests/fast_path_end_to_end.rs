use std::sync::Arc;

use a2ex_daemon::{
    DaemonConfig, DaemonService, SignerHandoff, load_runtime_state, spawn_local_daemon,
};
use a2ex_evm_adapter::{SimulatedEvmAdapter, SimulatedOutcome, TxLifecycleStatus};
use a2ex_ipc::JsonRpcRequest;
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::{ReservationManager, ReservationRequest, SqliteReservationManager};
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerIdentity, LocalPeerValidator, SignedTx, SignerBridge,
    SignerBridgeError, SignerBridgeRequestRecord, TxSignRequest,
};
use tempfile::tempdir;

#[derive(Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {}
}

#[derive(Default)]
struct SigningBridge;

#[async_trait::async_trait]
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
async fn fast_path_end_to_end_handles_allow_hold_fail_and_restart() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());

    let confirmed = execute_case(
        &config,
        "req-allow-generic",
        "intent_contract_call",
        "smart_contract",
        "polymarket",
        "reservation-generic",
        SimulatedOutcome::Confirmed,
    )
    .await;
    assert_eq!(confirmed, TxLifecycleStatus::Confirmed);

    let confirmed_simple = execute_case(
        &config,
        "req-allow-simple",
        "open_exposure",
        "prediction_market",
        "polymarket",
        "reservation-simple-e2e",
        SimulatedOutcome::Confirmed,
    )
    .await;
    assert_eq!(confirmed_simple, TxLifecycleStatus::Confirmed);

    let confirmed_hedge = execute_case(
        &config,
        "req-allow-hedge",
        "open_exposure",
        "prediction_market",
        "hyperliquid",
        "reservation-hedge-e2e",
        SimulatedOutcome::Confirmed,
    )
    .await;
    assert_eq!(confirmed_hedge, TxLifecycleStatus::Confirmed);

    let hold_config = DaemonConfig::for_data_dir(data_dir.path().join("hold"));
    tokio::fs::create_dir_all(hold_config.data_dir())
        .await
        .expect("hold data dir created");
    let hold_reservations = SqliteReservationManager::open(hold_config.state_db_path())
        .await
        .expect("reservation manager opens");
    let hold_service = DaemonService::from_config_with_fast_path(
        &hold_config,
        BaselinePolicy::new(100),
        hold_reservations,
        Arc::new(PassiveSigner),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 30,
            confirmation_depth: 1,
            outcome: SimulatedOutcome::Confirmed,
        },
    );
    hold_service
        .submit_intent(intent_request(
            "req-hold",
            "intent-hold",
            "prediction_market",
            "polymarket",
            true,
        ))
        .await
        .expect("hold request persists");
    assert!(
        hold_service
            .prepare_fast_path_action("req-hold", "reservation-hold")
            .await
            .is_err()
    );

    let failed = execute_case(
        &config,
        "req-fail",
        "open_exposure",
        "prediction_market",
        "polymarket",
        "reservation-fail",
        SimulatedOutcome::Failed {
            reason: "reverted".to_owned(),
        },
    )
    .await;
    assert_eq!(failed, TxLifecycleStatus::Failed);

    let confirmed_config = DaemonConfig::for_data_dir(config.data_dir().join("req-allow-generic"));
    let confirmed_daemon = spawn_local_daemon(confirmed_config.clone())
        .await
        .expect("confirmed daemon boots");
    confirmed_daemon
        .shutdown()
        .await
        .expect("confirmed daemon shuts down");
    let confirmed_snapshot = load_runtime_state(&confirmed_config)
        .await
        .expect("confirmed state reloads");
    assert!(
        confirmed_snapshot
            .executions
            .iter()
            .any(|record| record.status == "confirmed")
    );

    let failed_config = DaemonConfig::for_data_dir(config.data_dir().join("req-fail"));
    let failed_daemon = spawn_local_daemon(failed_config.clone())
        .await
        .expect("failed daemon boots");
    failed_daemon
        .shutdown()
        .await
        .expect("failed daemon shuts down");
    let failed_snapshot = load_runtime_state(&failed_config)
        .await
        .expect("failed state reloads");
    assert!(
        failed_snapshot
            .executions
            .iter()
            .any(|record| record.status == "failed")
    );
}

async fn execute_case(
    config: &DaemonConfig,
    request_id: &str,
    intent_type: &str,
    domain: &str,
    venue: &str,
    reservation_id: &str,
    outcome: SimulatedOutcome,
) -> TxLifecycleStatus {
    let case_dir = config.data_dir().join(request_id);
    let case_config = DaemonConfig::for_data_dir(case_dir);
    tokio::fs::create_dir_all(case_config.data_dir())
        .await
        .expect("case data dir created");
    let reservation_manager = SqliteReservationManager::open(case_config.state_db_path())
        .await
        .expect("reservation manager opens");
    reservation_manager
        .hold(ReservationRequest {
            reservation_id: reservation_id.to_owned(),
            execution_id: request_id.to_owned(),
            asset: "USDC".to_owned(),
            amount: 25,
        })
        .await
        .expect("reservation hold persists");
    let service = DaemonService::from_config_with_fast_path(
        &case_config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(SigningBridge),
            LocalPeerValidator::strict_local_only(),
        ),
        SimulatedEvmAdapter {
            block_number: 9,
            confirmation_depth: 1,
            outcome,
        },
    );

    service
        .submit_intent(intent_request(
            request_id,
            intent_type,
            domain,
            venue,
            false,
        ))
        .await
        .expect("intent persists");
    let prepared = service
        .prepare_fast_path_action(request_id, reservation_id)
        .await
        .expect("prepared action");
    let report = service
        .execute_fast_path_action(&prepared, LocalPeerIdentity::for_tests(true, true))
        .await
        .expect("execution reports lifecycle");

    report.terminal_status().expect("terminal status").clone()
}

fn intent_request(
    request_id: &str,
    intent_type: &str,
    domain: &str,
    venue: &str,
    preview_only: bool,
) -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        request_id,
        "daemon.submitIntent",
        serde_json::json!({
            "request_id": request_id,
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_id": format!("intent-{request_id}"),
                "intent_type": intent_type,
                "objective": {
                    "domain": domain,
                    "target_market": format!("market-{request_id}"),
                    "side": "yes",
                    "target_notional_usd": 25
                },
                "constraints": {
                    "allowed_venues": [venue],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "high"
                },
                "funding": {
                    "preferred_asset": "usdc",
                    "source_chain": "base"
                },
                "post_actions": []
            },
            "rationale": {
                "summary": "end to end fast path",
                "main_risks": []
            },
            "execution_preferences": {
                "preview_only": preview_only,
                "allow_fast_path": true
            }
        }),
    )
}
