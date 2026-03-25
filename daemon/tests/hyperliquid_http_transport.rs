//! Integration test: full place→sign→submit round-trip with wiremock.
//!
//! Exercises the real composition path: `HyperliquidAdapter` wrapping
//! `HyperliquidHttpTransport` wrapping a mock `SignerBridge`, against
//! wiremock HTTP mocks for both `/exchange` and `/info`.

use std::sync::{Arc, Mutex};

use a2ex_hyperliquid_adapter::HyperliquidHttpTransport;
use a2ex_hyperliquid_adapter::{
    HyperliquidAdapter, HyperliquidAdapterError, HyperliquidOrderCommand, HyperliquidSyncRequest,
};
use a2ex_signer_bridge::{SignedPayload, SignerBridge, SignerBridgeError, TypedDataSignRequest};
use async_trait::async_trait;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// MockSignerBridge — records requests, returns fixed signature
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct MockSignerState {
    requests: Vec<TypedDataSignRequest>,
}

#[derive(Debug, Clone, Default)]
struct MockSignerBridge {
    state: Arc<Mutex<MockSignerState>>,
}

impl MockSignerBridge {
    fn recorded_requests(&self) -> Vec<TypedDataSignRequest> {
        self.state.lock().unwrap().requests.clone()
    }
}

const FIXED_SIGNATURE: &str = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef1c";

#[async_trait]
impl SignerBridge for MockSignerBridge {
    async fn sign_typed_data(
        &self,
        req: TypedDataSignRequest,
    ) -> Result<SignedPayload, SignerBridgeError> {
        self.state.lock().unwrap().requests.push(req);
        Ok(SignedPayload::with_hex(FIXED_SIGNATURE.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_order_command() -> HyperliquidOrderCommand {
    HyperliquidOrderCommand {
        signer_address: "0xsigner".to_string(),
        account_address: "0xaccount".to_string(),
        asset: 4,
        is_buy: true,
        price: "30000.0".to_string(),
        size: "0.1".to_string(),
        reduce_only: false,
        client_order_id: Some("test-cloid".to_string()),
        time_in_force: "Gtc".to_string(),
    }
}

fn make_sync_request() -> HyperliquidSyncRequest {
    HyperliquidSyncRequest {
        signer_address: "0xsigner".to_string(),
        account_address: "0xaccount".to_string(),
        order_id: Some(555),
        aggregate_fills: false,
    }
}

async fn setup_exchange_success_mock(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/exchange"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "ok",
            "response": {
                "type": "order",
                "data": {
                    "statuses": [
                        { "resting": { "oid": 12345 } }
                    ]
                }
            }
        })))
        .expect(1)
        .mount(server)
        .await;
}

async fn setup_info_mocks(server: &MockServer) {
    // Open orders
    Mock::given(method("POST"))
        .and(path("/info"))
        .and(body_partial_json(json!({ "type": "openOrders" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "oid": 111,
                "asset": 4,
                "coin": "ETH",
                "side": "B",
                "limitPx": "3000.0",
                "sz": "1.5",
                "reduceOnly": false,
                "orderType": "Limit",
                "cloid": "my-order-1"
            }
        ])))
        .expect(1)
        .mount(server)
        .await;

    // Order status
    Mock::given(method("POST"))
        .and(path("/info"))
        .and(body_partial_json(json!({ "type": "orderStatus" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": {
                "order": { "oid": 555, "filledSz": "0.25" },
                "status": "open"
            }
        })))
        .expect(1)
        .mount(server)
        .await;

    // User fills
    Mock::given(method("POST"))
        .and(path("/info"))
        .and(body_partial_json(json!({ "type": "userFills" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "oid": 777,
                "asset": 4,
                "coin": "BTC",
                "sz": "0.01",
                "px": "60000.0",
                "side": "B",
                "time": "2026-01-01T00:00:00Z"
            }
        ])))
        .expect(1)
        .mount(server)
        .await;

    // Clearinghouse state
    Mock::given(method("POST"))
        .and(path("/info"))
        .and(body_partial_json(json!({ "type": "clearinghouseState" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "assetPositions": [
                {
                    "position": {
                        "asset": 4,
                        "coin": "ETH",
                        "szi": "2.5",
                        "entryPx": "3100.0",
                        "positionValue": "7750.0"
                    }
                }
            ]
        })))
        .expect(1)
        .mount(server)
        .await;
}

// ---------------------------------------------------------------------------
// Test: place_order full round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn place_order_full_round_trip() {
    let server = MockServer::start().await;
    setup_exchange_success_mock(&server).await;

    let mock_signer = MockSignerBridge::default();
    let transport = HyperliquidHttpTransport::new(
        server.uri(),
        Arc::new(mock_signer.clone()),
        true, // mainnet
    );
    let adapter = HyperliquidAdapter::with_transport(Arc::new(transport), 1000);

    let ack = adapter
        .place_order(make_order_command())
        .await
        .expect("place_order should succeed");

    // Verify adapter-level response
    assert_eq!(ack.signer_address, "0xsigner");
    assert_eq!(ack.account_address, "0xaccount");
    assert_eq!(ack.status, "ok");
    assert_eq!(ack.order_id, Some(12345));

    // Verify wiremock received correctly shaped request
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(body["nonce"].is_number(), "nonce must be present");
    // Signature is now an {r, s, v} object (parsed from hex) or fallback string
    assert!(
        body["signature"].is_object() || body["signature"].is_string(),
        "signature must be object or string"
    );
    assert!(body["vaultAddress"].is_null(), "vaultAddress must be null");

    let action = &body["action"];
    assert_eq!(action["type"], "order");
    assert_eq!(action["grouping"], "na");
    assert!(action["orders"].is_array());
    assert_eq!(action["orders"][0]["a"], 4);
    assert_eq!(action["orders"][0]["b"], true);
    assert_eq!(action["orders"][0]["p"], "30000.0");
    assert_eq!(action["orders"][0]["s"], "0.1");

    // Verify MockSignerBridge received EIP-712 request with chainId 1337 and Agent type
    let sign_requests = mock_signer.recorded_requests();
    assert_eq!(
        sign_requests.len(),
        1,
        "signer should be called exactly once"
    );

    let sign_req = &sign_requests[0];
    let domain = sign_req.domain.as_ref().expect("domain must be set");
    assert_eq!(domain.chain_id, Some(1337), "chainId must be 1337");
    assert_eq!(domain.name.as_deref(), Some("Exchange"));
    assert_eq!(domain.version.as_deref(), Some("1"));
    assert_eq!(
        domain.verifying_contract.as_deref(),
        Some("0x0000000000000000000000000000000000000000")
    );

    assert_eq!(sign_req.primary_type.as_deref(), Some("Agent"));

    let types = sign_req.types.as_ref().expect("types must be set");
    assert!(types.get("Agent").is_some(), "Agent type must be present");

    let message = sign_req.message.as_ref().expect("message must be set");
    assert_eq!(
        message["source"].as_str(),
        Some("a"),
        "mainnet source must be 'a'"
    );
    assert!(
        message["connectionId"].as_str().unwrap().starts_with("0x"),
        "connectionId must be 0x-prefixed"
    );
}

// ---------------------------------------------------------------------------
// Test: sync_state full round-trip (all four info request types)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_state_full_round_trip() {
    let server = MockServer::start().await;
    setup_info_mocks(&server).await;

    let mock_signer = MockSignerBridge::default();
    let transport =
        HyperliquidHttpTransport::new(server.uri(), Arc::new(mock_signer.clone()), true);
    let adapter = HyperliquidAdapter::with_transport(Arc::new(transport), 0);

    let snapshot = adapter
        .sync_state(make_sync_request())
        .await
        .expect("sync_state should succeed");

    // Verify all fields populated from the four info endpoints
    assert_eq!(snapshot.queried_account, "0xaccount");
    assert_eq!(snapshot.queried_signer, "0xsigner");

    // Open orders
    assert_eq!(snapshot.open_orders.len(), 1);
    assert_eq!(snapshot.open_orders[0].order_id, 111);
    assert_eq!(snapshot.open_orders[0].instrument, "ETH");
    assert!(snapshot.open_orders[0].is_buy);
    assert_eq!(snapshot.open_orders[0].price, "3000.0");
    assert_eq!(snapshot.open_orders[0].size, "1.5");
    assert_eq!(
        snapshot.open_orders[0].client_order_id.as_deref(),
        Some("my-order-1")
    );

    // Order status
    let status = snapshot.order_status.expect("order_status should be Some");
    assert_eq!(status.order_id, 555);
    assert_eq!(status.status, "open");
    assert_eq!(status.filled_size, "0.25");

    // Fills
    assert_eq!(snapshot.fills.len(), 1);
    assert_eq!(snapshot.fills[0].order_id, 777);
    assert_eq!(snapshot.fills[0].instrument, "BTC");
    assert_eq!(snapshot.fills[0].price, "60000.0");
    assert_eq!(snapshot.fills[0].side, "B");

    // Positions
    assert_eq!(snapshot.positions.len(), 1);
    assert_eq!(snapshot.positions[0].asset, 4);
    assert_eq!(snapshot.positions[0].instrument, "ETH");
    assert_eq!(snapshot.positions[0].size, "2.5");
    assert_eq!(snapshot.positions[0].entry_price, "3100.0");
    assert_eq!(snapshot.positions[0].position_value, "7750.0");

    // Verify all four info requests hit wiremock
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4, "should make exactly 4 info requests");

    let request_types: Vec<String> = requests
        .iter()
        .map(|r| {
            let body: Value = serde_json::from_slice(&r.body).unwrap();
            body["type"].as_str().unwrap().to_string()
        })
        .collect();

    assert!(request_types.contains(&"openOrders".to_string()));
    assert!(request_types.contains(&"orderStatus".to_string()));
    assert!(request_types.contains(&"userFills".to_string()));
    assert!(request_types.contains(&"clearinghouseState".to_string()));

    // Verify signer was NOT called for info queries (unauthenticated)
    assert!(
        mock_signer.recorded_requests().is_empty(),
        "info queries must not invoke the signer"
    );
}

// ---------------------------------------------------------------------------
// Test: exchange error handling (200 with error in statuses)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn place_order_exchange_error_in_statuses() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/exchange"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "ok",
            "response": {
                "type": "order",
                "data": {
                    "statuses": [
                        "Error: Insufficient margin"
                    ]
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mock_signer = MockSignerBridge::default();
    let transport = HyperliquidHttpTransport::new(server.uri(), Arc::new(mock_signer), true);
    let adapter = HyperliquidAdapter::with_transport(Arc::new(transport), 0);

    let err = adapter
        .place_order(make_order_command())
        .await
        .expect_err("should return error for error-in-statuses");

    match err {
        HyperliquidAdapterError::Transport { message } => {
            assert!(
                message.contains("Insufficient margin"),
                "error should contain exchange rejection text: {message}"
            );
        }
    }
}
