mod support;

use std::sync::{Arc, Mutex};

use a2ex_daemon::{DaemonConfig, DaemonService, NoopRuntimeSigner};
use a2ex_policy::BaselinePolicy;
use a2ex_reservation::SqliteReservationManager;
use a2ex_signer_bridge::{
    ApprovalRequest, ApprovalResult, LocalPeerValidator, SignerBridge, SignerBridgeRequestRecord,
};
use async_trait::async_trait;
use serde_json::json;
use tempfile::tempdir;

#[derive(Default)]
struct RecordingApprovalBridge {
    approvals: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl SignerBridge for RecordingApprovalBridge {
    async fn request_approval(
        &self,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, a2ex_signer_bridge::SignerBridgeError> {
        self.approvals
            .lock()
            .expect("approvals lock")
            .push(req.action_kind.clone());
        Ok(ApprovalResult {
            approved: true,
            audit: SignerBridgeRequestRecord::approval(req),
        })
    }
}

#[tokio::test]
async fn planner_persists_multistep_plan_and_capability_matrix() {
    let data_dir = tempdir().expect("temp dir");
    let config = DaemonConfig::for_data_dir(data_dir.path());
    let service = DaemonService::from_config_with_signer_bridge(
        &config,
        BaselinePolicy::new(10_000),
        SqliteReservationManager::open(config.state_db_path())
            .await
            .expect("reservations"),
        Arc::new(NoopRuntimeSigner),
        a2ex_signer_bridge::LocalSignerBridgeClient::new(
            Arc::new(RecordingApprovalBridge::default()),
            LocalPeerValidator::strict_local_only(),
        ),
    );

    let submit = service
        .submit_intent(intent_request())
        .await
        .expect("submit intent");
    assert!(matches!(submit, a2ex_ipc::JsonRpcResponse::Success(_)));

    let plan = service
        .plan_intent_request("req-plan-1")
        .await
        .expect("plan intent");
    let matrix = service.capability_matrix();

    assert_eq!(plan.route.bridge_venue.as_deref(), Some("across"));
    assert_eq!(plan.route.entry_venue, "polymarket");
    assert_eq!(plan.route.fallback_entry_venue.as_deref(), Some("kalshi"));
    assert_eq!(plan.route.hedge_venue.as_deref(), Some("hyperliquid"));
    assert_eq!(plan.steps.len(), 3);
    assert!(matrix.venue("across").is_some());
    assert!(
        matrix
            .venue("polymarket")
            .expect("polymarket capability")
            .approval_requirements
            .iter()
            .any(|requirement| requirement.required)
    );
}

fn intent_request() -> a2ex_ipc::JsonRpcRequest<serde_json::Value> {
    a2ex_ipc::JsonRpcRequest::new(
        "req-plan-1",
        "daemon.submitIntent",
        json!({
            "request_id": "req-plan-1",
            "request_kind": "intent",
            "source_agent_id": "agent-main",
            "submitted_at": "2026-03-12T00:00:00Z",
            "payload": {
                "intent_id": "intent-multi-1",
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
