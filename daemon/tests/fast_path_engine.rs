use std::sync::Arc;

use a2ex_daemon::{DaemonConfig, DaemonService, SignerHandoff};
use a2ex_fast_path::PreparedVenueAction;
use a2ex_ipc::JsonRpcRequest;
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use tempfile::tempdir;

#[derive(Default)]
struct PassiveSigner;

impl SignerHandoff for PassiveSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {}
}

#[tokio::test]
async fn fast_path_template_expands_into_prepared_action() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
    );

    service
        .submit_intent(fast_intent_request(
            "req-fast-simple",
            "intent-fast-simple",
            "polymarket",
        ))
        .await
        .expect("intent persists");

    let prepared = service
        .prepare_fast_path_action("req-fast-simple", "reservation-fast-simple")
        .await
        .expect("fast path prepares");

    assert_eq!(prepared.request_id, "req-fast-simple");
    assert_eq!(prepared.reservation_id, "reservation-fast-simple");
    assert_eq!(prepared.action_kind, "simple_entry");
    assert_eq!(prepared.venue, "polymarket");
    assert!(matches!(
        prepared.payload,
        PreparedVenueAction::SimpleEntry { .. }
    ));
}

#[tokio::test]
async fn fast_path_engine_requires_persisted_fast_path_route_context() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(PassiveSigner),
    );

    service
        .submit_intent(planned_intent_request())
        .await
        .expect("planned intent persists");

    let error = service
        .prepare_fast_path_action("req-planned", "reservation-planned")
        .await
        .expect_err("non-fast route should fail");

    assert!(error.to_string().contains("not fast_path"));
}

fn fast_intent_request(
    request_id: &str,
    intent_id: &str,
    venue: &str,
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
                "intent_id": intent_id,
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "market-1",
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
                "summary": "fast path request",
                "main_risks": []
            },
            "execution_preferences": {
                "preview_only": false,
                "allow_fast_path": true
            }
        }),
    )
}

fn planned_intent_request() -> JsonRpcRequest<serde_json::Value> {
    JsonRpcRequest::new(
        "req-planned",
        "daemon.submitIntent",
        serde_json::json!({
            "request_id": "req-planned",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_id": "intent-planned",
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "market-2",
                    "side": "yes",
                    "target_notional_usd": 25
                },
                "constraints": {
                    "allowed_venues": ["polymarket", "kalshi"],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "high",
                    "hedge_ratio_bps": 4000
                },
                "funding": {
                    "preferred_asset": "usdc",
                    "source_chain": "base"
                },
                "post_actions": []
            },
            "rationale": {
                "summary": "planned request",
                "main_risks": []
            },
            "execution_preferences": {
                "preview_only": false,
                "allow_fast_path": true
            }
        }),
    )
}
