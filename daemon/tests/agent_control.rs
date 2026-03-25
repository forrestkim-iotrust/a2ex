use std::sync::Arc;

use a2ex_control::{
    AgentRequestEnvelope, AgentRequestKind, ExecutionPreferences, ExecutionUrgency, Intent,
    IntentConstraints, IntentFunding, IntentObjective, RationaleSummary,
};
use a2ex_daemon::{DaemonConfig, DaemonService, IntentSubmissionReceipt, SignerHandoff};
use a2ex_ipc::{JsonRpcRequest, JsonRpcResponse};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_state::StateRepository;
use serde_json::json;
use tempfile::tempdir;

#[derive(Default)]
struct RecordingSigner {
    handoff_count: std::sync::atomic::AtomicUsize,
}

impl SignerHandoff for RecordingSigner {
    fn handoff(&self, _request: &a2ex_daemon::ExecutionRequest) {
        self.handoff_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn submit_intent_persists_rationale_and_preferences() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let signer = Arc::new(RecordingSigner::default());
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        signer.clone(),
    );

    let request = JsonRpcRequest::new(
        "req-intent-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-intent-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_id": "intent-1",
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
                "post_actions": []
            },
            "rationale": {
                "summary": "Opportunity remains positive after costs.",
                "main_risks": ["spread compression"]
            },
            "execution_preferences": {
                "preview_only": false,
                "allow_fast_path": true
            }
        }),
    );

    let response: JsonRpcResponse<IntentSubmissionReceipt> = service
        .submit_intent(request)
        .await
        .expect("submit intent succeeds");

    match response {
        JsonRpcResponse::Success(success) => {
            assert_eq!(success.id, "req-intent-1");
            assert_eq!(success.result.request_id, "req-intent-1");
            assert_eq!(success.result.intent_id, "intent-1");
        }
        JsonRpcResponse::Failure(failure) => panic!("expected success, got {:?}", failure.error),
    }

    assert_eq!(
        service.recorded_intents(),
        vec![AgentRequestEnvelope {
            request_id: "req-intent-1".to_owned(),
            request_kind: AgentRequestKind::Intent,
            source_agent_id: "agent-main".to_owned(),
            submitted_at: "2026-03-11T00:00:00Z".to_owned(),
            payload: Intent {
                intent_id: "intent-1".to_owned(),
                intent_type: "open_exposure".to_owned(),
                objective: IntentObjective {
                    domain: "prediction_market".to_owned(),
                    target_market: "us-election-2028".to_owned(),
                    side: "yes".to_owned(),
                    target_notional_usd: 3000,
                },
                constraints: IntentConstraints {
                    allowed_venues: vec!["polymarket".to_owned(), "kalshi".to_owned()],
                    max_slippage_bps: 80,
                    max_fee_usd: 25,
                    urgency: ExecutionUrgency::Normal,
                    hedge_ratio_bps: Some(4000),
                },
                funding: IntentFunding {
                    preferred_asset: "USDC".to_owned(),
                    source_chain: "base".to_owned(),
                },
                post_actions: vec![],
            },
            rationale: RationaleSummary {
                summary: "Opportunity remains positive after costs.".to_owned(),
                main_risks: vec!["spread compression".to_owned()],
            },
            execution_preferences: ExecutionPreferences {
                preview_only: false,
                allow_fast_path: true,
                client_request_label: None,
            },
        }]
    );

    let repository = StateRepository::open(config.state_db_path())
        .await
        .expect("state repository opens");
    let stored = repository
        .load_intent_submission("intent-1")
        .await
        .expect("intent query succeeds")
        .expect("intent persisted");

    assert_eq!(stored.request_id, "req-intent-1");
    assert_eq!(stored.source_agent_id, "agent-main");
    assert_eq!(stored.intent_type, "open_exposure");
    assert_eq!(
        stored.rationale.summary,
        "Opportunity remains positive after costs."
    );
    assert_eq!(
        stored.rationale.main_risks,
        vec!["spread compression".to_owned()]
    );
    assert_eq!(stored.execution_preferences.allow_fast_path, true);
    assert_eq!(stored.execution_preferences.preview_only, false);

    let journal = repository.load_journal().await.expect("journal loads");
    let persisted_event = journal
        .iter()
        .find(|entry| entry.event_type == "intent_submitted" && entry.stream_id == "intent-1")
        .expect("intent journal entry exists");
    assert!(
        persisted_event
            .payload_json
            .contains("Opportunity remains positive after costs.")
    );
    assert!(persisted_event.payload_json.contains("allow_fast_path"));

    let snapshot = repository.load_snapshot().await.expect("snapshot loads");
    assert!(
        snapshot.executions.is_empty(),
        "intent submission should not create execution state"
    );
    assert_eq!(
        signer
            .handoff_count
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "intent submission should not hand off to signer"
    );
}

#[tokio::test]
async fn malformed_intent_envelope_fails_before_recording() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let reservation_manager = SqliteReservationManager::open(config.state_db_path())
        .await
        .expect("reservation manager opens");
    let service = DaemonService::from_config(
        &config,
        BaselinePolicy::new(100),
        reservation_manager,
        Arc::new(RecordingSigner::default()),
    );

    let request = JsonRpcRequest::new(
        "req-intent-bad",
        "daemon.submitIntent",
        json!({
            "request_id": "req-intent-bad",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-11T00:00:00Z",
            "payload": {
                "intent_type": "open_exposure",
                "objective": {
                    "domain": "prediction_market",
                    "target_market": "us-election-2028",
                    "side": "yes",
                    "target_notional_usd": 3000
                },
                "constraints": {
                    "allowed_venues": ["polymarket"],
                    "max_slippage_bps": 80,
                    "max_fee_usd": 25,
                    "urgency": "normal"
                },
                "funding": {
                    "preferred_asset": "USDC",
                    "source_chain": "base"
                },
                "post_actions": []
            },
            "rationale": {
                "summary": "Opportunity remains positive after costs.",
                "main_risks": []
            },
            "execution_preferences": {
                "preview_only": false,
                "allow_fast_path": true
            }
        }),
    );

    let response: JsonRpcResponse<IntentSubmissionReceipt> = service
        .submit_intent(request)
        .await
        .expect("submit intent returns response");

    match response {
        JsonRpcResponse::Failure(failure) => {
            assert_eq!(failure.id, "req-intent-bad");
            assert!(failure.error.message.contains("payload"));
        }
        JsonRpcResponse::Success(success) => {
            panic!("expected decode failure, got {:?}", success.result)
        }
    }

    assert!(service.recorded_intents().is_empty());
}
