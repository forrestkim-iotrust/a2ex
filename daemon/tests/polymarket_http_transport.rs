//! Integration test: full adapter round-trip through wiremock for Polymarket.
//!
//! Exercises the real composition path: `PredictionMarketAdapter` wrapping
//! `PolymarketHttpTransport` wrapping a mock `SignerBridge`, against
//! wiremock HTTP mocks for `POST /order` and `GET /data/order/{id}`.

use std::sync::{Arc, Mutex};

use a2ex_prediction_market_adapter::{
    PolymarketApiCredentials, PolymarketHttpTransport, PredictionAuth, PredictionMarketAdapter,
    PredictionMarketAdapterError, PredictionMarketTransport, PredictionOrderRequest,
    PredictionVenue,
};
use a2ex_signer_bridge::{SignedPayload, SignerBridge, SignerBridgeError, TypedDataSignRequest};
use async_trait::async_trait;
use base64::{
    Engine, engine::general_purpose::STANDARD as BASE64,
    engine::general_purpose::URL_SAFE as BASE64_URL_SAFE,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
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

fn test_credentials() -> PolymarketApiCredentials {
    let secret_raw = b"test-secret-bytes-for-hmac";
    PolymarketApiCredentials {
        api_key: "test-api-key".to_string(),
        secret: BASE64.encode(secret_raw),
        passphrase: "test-passphrase".to_string(),
    }
}

fn make_transport(base_url: &str, signer: Arc<dyn SignerBridge>) -> PolymarketHttpTransport {
    PolymarketHttpTransport::new(base_url, signer, test_credentials(), "0xWalletAddr")
}

fn test_order_request() -> PredictionOrderRequest {
    PredictionOrderRequest {
        venue: PredictionVenue::Polymarket,
        market: "71321045649".to_string(),
        side: "BUY".to_string(),
        size: "10".to_string(),
        price: "0.52".to_string(),
        max_fee_bps: 100,
        max_slippage_bps: 50,
        idempotency_key: "test-idemp-key".to_string(),
        auth: PredictionAuth {
            credential_id: "cred-1".to_string(),
            auth_summary: "test".to_string(),
        },
    }
}

async fn setup_order_success_mock(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "orderID": "poly-order-001",
            "status": "live",
        })))
        .expect(1)
        .mount(server)
        .await;
}

// ---------------------------------------------------------------------------
// (a) place_order_success — verify PredictionOrderAck fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn place_order_success() {
    let server = MockServer::start().await;
    setup_order_success_mock(&server).await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer));

    let ack = transport.place_order(test_order_request()).await.unwrap();
    assert_eq!(ack.venue, PredictionVenue::Polymarket);
    assert_eq!(ack.order_id, "poly-order-001");
    assert_eq!(ack.status, "live");
    assert_eq!(ack.idempotency_key, "test-idemp-key");
}

// ---------------------------------------------------------------------------
// (b) place_order_body_shape — verify request body JSON fields + EIP-712 sig
// ---------------------------------------------------------------------------

#[tokio::test]
async fn place_order_body_shape() {
    let server = MockServer::start().await;
    setup_order_success_mock(&server).await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer));

    transport.place_order(test_order_request()).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    let body: Value = serde_json::from_slice(&requests[0].body).expect("valid JSON body");

    // Top-level fields
    assert!(body["signature"].is_string(), "signature must be present");
    assert_eq!(
        body["signature"].as_str().unwrap(),
        FIXED_SIGNATURE,
        "signature must be the fixed mock value"
    );
    assert_eq!(body["owner"], "0xWalletAddr");
    assert_eq!(body["orderType"], "GTC");

    // Order sub-object
    let order = &body["order"];
    assert!(order.is_object(), "order must be an object");
    assert_eq!(order["maker"], "0xWalletAddr");
    assert_eq!(order["signer"], "0xWalletAddr");
    assert_eq!(order["taker"], "0x0000000000000000000000000000000000000000");
    assert_eq!(order["tokenId"], "71321045649");
    assert_eq!(order["side"], "0"); // BUY = 0
    assert!(order["salt"].is_string(), "salt must be present");
    assert_eq!(order["makerAmount"], "5200000");
    assert_eq!(order["takerAmount"], "10000000");
    assert_eq!(order["expiration"], "0");
    assert_eq!(order["nonce"], "0");
    assert_eq!(order["signatureType"], "0");
}

// ---------------------------------------------------------------------------
// (c) place_order_l2_headers — verify all 5 HMAC headers present
// ---------------------------------------------------------------------------

#[tokio::test]
async fn place_order_l2_headers() {
    let server = MockServer::start().await;
    setup_order_success_mock(&server).await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer));

    transport.place_order(test_order_request()).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    let headers = &requests[0].headers;

    let poly_address = headers.get("POLY_ADDRESS").expect("POLY_ADDRESS missing");
    assert_eq!(poly_address.to_str().unwrap(), "0xWalletAddr");

    let poly_sig = headers
        .get("POLY_SIGNATURE")
        .expect("POLY_SIGNATURE missing");
    assert!(
        !poly_sig.to_str().unwrap().is_empty(),
        "POLY_SIGNATURE must be non-empty"
    );
    BASE64_URL_SAFE
        .decode(poly_sig.to_str().unwrap())
        .expect("POLY_SIGNATURE must be valid base64");

    let poly_ts = headers
        .get("POLY_TIMESTAMP")
        .expect("POLY_TIMESTAMP missing");
    let ts_str = poly_ts.to_str().unwrap();
    assert!(!ts_str.is_empty(), "POLY_TIMESTAMP must be non-empty");
    ts_str
        .parse::<u64>()
        .expect("POLY_TIMESTAMP must be a numeric unix timestamp");

    let poly_key = headers.get("POLY_API_KEY").expect("POLY_API_KEY missing");
    assert_eq!(poly_key.to_str().unwrap(), "test-api-key");

    let poly_pass = headers
        .get("POLY_PASSPHRASE")
        .expect("POLY_PASSPHRASE missing");
    assert_eq!(poly_pass.to_str().unwrap(), "test-passphrase");
}

// ---------------------------------------------------------------------------
// (d) sync_order_success — verify PredictionOrderStatus fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_order_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/data/order/order-789"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "matched",
            "filledSize": "50",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer));

    let status = transport
        .sync_order(PredictionVenue::Polymarket, "order-789")
        .await
        .unwrap();

    assert_eq!(status.venue, PredictionVenue::Polymarket);
    assert_eq!(status.order_id, "order-789");
    assert_eq!(status.status, "matched");
    assert_eq!(status.filled_amount, "50");

    // Verify HMAC headers on the GET request
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let headers = &requests[0].headers;
    assert!(headers.get("POLY_ADDRESS").is_some());
    assert!(headers.get("POLY_SIGNATURE").is_some());
    assert!(headers.get("POLY_TIMESTAMP").is_some());
    assert!(headers.get("POLY_API_KEY").is_some());
    assert!(headers.get("POLY_PASSPHRASE").is_some());
}

// ---------------------------------------------------------------------------
// (e) place_and_sync_round_trip — full PredictionMarketAdapter round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn place_and_sync_round_trip() {
    let server = MockServer::start().await;

    // POST /order mock
    Mock::given(method("POST"))
        .and(path("/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "orderID": "round-trip-001",
            "status": "live",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // GET /data/order/round-trip-001 mock
    Mock::given(method("GET"))
        .and(path("/data/order/round-trip-001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "matched",
            "filledSize": "75",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer.clone()));
    let adapter = PredictionMarketAdapter::with_transport(Arc::new(transport));

    let (ack, status) = adapter
        .place_and_sync(test_order_request())
        .await
        .expect("place_and_sync should succeed");

    // Verify ack
    assert_eq!(ack.venue, PredictionVenue::Polymarket);
    assert_eq!(ack.order_id, "round-trip-001");
    assert_eq!(ack.status, "live");
    assert_eq!(ack.idempotency_key, "test-idemp-key");

    // Verify status
    assert_eq!(status.venue, PredictionVenue::Polymarket);
    assert_eq!(status.order_id, "round-trip-001");
    assert_eq!(status.status, "matched");
    assert_eq!(status.filled_amount, "75");

    // Verify wiremock received both requests (POST + GET)
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        2,
        "should make exactly 2 requests (place + sync)"
    );

    // Verify signer was called exactly once (only for place_order EIP-712)
    let sign_requests = mock_signer.recorded_requests();
    assert_eq!(
        sign_requests.len(),
        1,
        "signer should be called once for place_order, not for sync_order"
    );
}

// ---------------------------------------------------------------------------
// (f) clob_rejection_error — CLOB error JSON propagates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clob_rejection_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "insufficient balance",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer));

    let err = transport
        .place_order(test_order_request())
        .await
        .expect_err("should return error for CLOB rejection");

    match err {
        PredictionMarketAdapterError::Transport { message } => {
            assert!(
                message.contains("CLOB rejected"),
                "error should mention CLOB rejection: {message}"
            );
            assert!(
                message.contains("insufficient balance"),
                "error should contain CLOB error text: {message}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// (g) http_500_error — HTTP error mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_500_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/order"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
        .expect(1)
        .mount(&server)
        .await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer));

    let err = transport
        .place_order(test_order_request())
        .await
        .expect_err("should return error for HTTP 500");

    match err {
        PredictionMarketAdapterError::Transport { message } => {
            assert!(
                message.contains("500"),
                "error should contain HTTP status: {message}"
            );
            assert!(
                message.contains("CLOB HTTP"),
                "error should identify as CLOB HTTP error: {message}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// (h) signer_records_eip712_request — verify EIP-712 Order domain/types
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signer_records_eip712_request() {
    let server = MockServer::start().await;
    setup_order_success_mock(&server).await;

    let mock_signer = MockSignerBridge::default();
    let transport = make_transport(&server.uri(), Arc::new(mock_signer.clone()));

    transport.place_order(test_order_request()).await.unwrap();

    let sign_requests = mock_signer.recorded_requests();
    assert_eq!(
        sign_requests.len(),
        1,
        "signer should be called exactly once"
    );

    let sign_req = &sign_requests[0];

    // Verify EIP-712 domain
    let domain = sign_req.domain.as_ref().expect("domain must be set");
    assert_eq!(
        domain.name.as_deref(),
        Some("ClobAuthDomain"),
        "domain name must be ClobAuthDomain"
    );
    assert_eq!(
        domain.version.as_deref(),
        Some("1"),
        "domain version must be 1"
    );
    assert_eq!(
        domain.chain_id,
        Some(137),
        "chainId must be 137 (Polygon mainnet)"
    );
    assert!(
        domain.verifying_contract.is_some(),
        "verifyingContract must be set to CTF Exchange address"
    );

    // Verify primary type is "Order"
    assert_eq!(
        sign_req.primary_type.as_deref(),
        Some("Order"),
        "primaryType must be Order"
    );

    // Verify types contain "Order" fields (types is serde_json::Value)
    let types = sign_req.types.as_ref().expect("types must be set");
    let order_type = types.get("Order").expect("Order type must be present");
    let order_fields = order_type.as_array().expect("Order type must be an array");
    let field_names: Vec<&str> = order_fields
        .iter()
        .filter_map(|f| f.get("name").and_then(|n| n.as_str()))
        .collect();
    assert!(field_names.contains(&"salt"), "Order must have salt field");
    assert!(
        field_names.contains(&"maker"),
        "Order must have maker field"
    );
    assert!(
        field_names.contains(&"taker"),
        "Order must have taker field"
    );
    assert!(
        field_names.contains(&"tokenId"),
        "Order must have tokenId field"
    );
    assert!(
        field_names.contains(&"makerAmount"),
        "Order must have makerAmount field"
    );
    assert!(
        field_names.contains(&"takerAmount"),
        "Order must have takerAmount field"
    );

    // Verify message contains the order fields
    let message = sign_req.message.as_ref().expect("message must be set");
    assert_eq!(message["maker"], "0xWalletAddr");
    assert_eq!(message["tokenId"], "71321045649");
    assert_eq!(message["side"], "0"); // BUY
}
