//! Integration tests for `AcrossHttpTransport` against wiremock.
//!
//! Exercises quote, submit_bridge, and error handling against mock HTTP
//! endpoints simulating the Across Swap API.

use std::sync::Arc;

use a2ex_across_adapter::{
    AcrossAdapter, AcrossApproval, AcrossBridgeQuote, AcrossBridgeQuoteRequest,
    AcrossBridgeRequest, AcrossTransport, SwapTx, transport::AcrossHttpTransport,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Fixture JSON constants
// ---------------------------------------------------------------------------

fn swap_approval_response() -> Value {
    json!({
        "crossSwapType": "bridgeableToBridgeable",
        "amountType": "exactInput",
        "inputAmount": "1000000",
        "expectedOutputAmount": "999500",
        "minOutputAmount": "998000",
        "expectedFillTime": 30,
        "quoteExpiryTimestamp": 1700003600,
        "checks": {
            "allowance": {
                "token": "0xUSDC",
                "spender": "0xSpokePool"
            }
        },
        "approvalTxns": [
            {
                "chainId": 1,
                "to": "0xApprovalTarget",
                "data": "0xapprove_data"
            }
        ],
        "swapTx": {
            "ecosystem": "evm",
            "chainId": 1,
            "to": "0xSpokePool",
            "data": "0xswap_calldata_here",
            "gas": "0"
        },
        "inputToken": {
            "address": "0xUSDC",
            "symbol": "USDC",
            "decimals": 6,
            "chainId": 1
        },
        "outputToken": {
            "address": "0xUSDC_dst",
            "symbol": "USDC",
            "decimals": 6,
            "chainId": 42161
        },
        "fees": {
            "total": {
                "amount": "500",
                "amountUsd": "0.50"
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_quote_request() -> AcrossBridgeQuoteRequest {
    AcrossBridgeQuoteRequest {
        asset: "0xUSDC".to_string(),
        amount_usd: 1000000,
        source_chain: "1".to_string(),
        destination_chain: "42161".to_string(),
        depositor: None,
        recipient: None,
        output_token: None,
    }
}

fn build_transport(base_url: &str) -> AcrossHttpTransport {
    AcrossHttpTransport::new(
        base_url,
        Some("test-integrator".into()),
        Some("test-key".into()),
    )
}

fn build_transport_no_auth(base_url: &str) -> AcrossHttpTransport {
    AcrossHttpTransport::new(base_url, None, None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_quote_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/swap/approval"))
        .and(query_param("tradeType", "exactInput"))
        .and(query_param("amount", "1000000"))
        .and(query_param("inputToken", "0xUSDC"))
        .and(query_param("originChainId", "1"))
        .and(query_param("destinationChainId", "42161"))
        .and(query_param("integratorId", "test-integrator"))
        .respond_with(ResponseTemplate::new(200).set_body_json(swap_approval_response()))
        .expect(1)
        .mount(&server)
        .await;

    let transport = build_transport(&server.uri());
    let quote = transport
        .quote(make_quote_request())
        .await
        .expect("quote should succeed");

    // Verify enriched fields from Swap API response
    assert_eq!(quote.route_id, "across-1-42161");
    assert_eq!(quote.bridge_fee_usd, 500);
    assert_eq!(quote.expected_fill_seconds, 30);

    // Approval info
    assert_eq!(quote.approval.token, "0xUSDC");
    assert_eq!(quote.approval.spender, "0xSpokePool");
    // In the real API format, allowance_target comes from checks.allowance.token
    assert_eq!(quote.approval.allowance_target, "0xUSDC");

    // Calldata from swap step
    assert_eq!(quote.calldata.as_deref(), Some("0xswap_calldata_here"));

    // SwapTx
    let swap_tx = quote.swap_tx.as_ref().expect("swap_tx should be populated");
    assert_eq!(swap_tx.to, "0xSpokePool");
    assert_eq!(swap_tx.data, "0xswap_calldata_here");
    assert_eq!(swap_tx.value, "0");

    // Token amounts
    assert_eq!(quote.input_amount.as_deref(), Some("1000000"));
    assert_eq!(quote.output_amount.as_deref(), Some("999500"));

    // Quote expiry from fill_deadline
    assert_eq!(quote.quote_expiry_secs, Some(1700003600));
}

#[tokio::test]
async fn test_submit_bridge_returns_calldata() {
    let server = MockServer::start().await;
    let transport = build_transport(&server.uri());

    // Build a bridge request with a pre-populated quote containing calldata.
    let bridge_req = AcrossBridgeRequest {
        deposit_id: "across-deposit-1".to_string(),
        signer_address: "0xSigner".to_string(),
        recipient_address: "0xRecipient".to_string(),
        asset: "USDC".to_string(),
        amount_usd: 1000,
        source_chain: "1".to_string(),
        destination_chain: "42161".to_string(),
        quote: AcrossBridgeQuote {
            route_id: "across-1-42161".to_string(),
            bridge_fee_usd: 500,
            expected_fill_seconds: 30,
            approval: AcrossApproval {
                token: "0xUSDC".to_string(),
                spender: "0xSpokePool".to_string(),
                allowance_target: "0xUSDC".to_string(),
            },
            calldata: Some("0xswap_calldata_here".to_string()),
            swap_tx: Some(SwapTx {
                to: "0xSpokePool".to_string(),
                data: "0xswap_calldata_here".to_string(),
                value: "0".to_string(),
            }),
            input_amount: Some("1000000".to_string()),
            output_amount: Some("999500".to_string()),
            quote_expiry_secs: Some(1700003600),
        },
    };

    let ack = transport
        .submit_bridge(bridge_req)
        .await
        .expect("submit_bridge should succeed");

    // Verify the ack carries calldata from the quote (no HTTP call made).
    assert_eq!(ack.deposit_id, "across-deposit-1");
    assert_eq!(ack.status, "pending");
    assert_eq!(ack.route_id, "across-1-42161");
    assert_eq!(ack.calldata.as_deref(), Some("0xswap_calldata_here"));

    let swap_tx = ack.swap_tx.as_ref().expect("swap_tx should be in ack");
    assert_eq!(swap_tx.to, "0xSpokePool");
    assert_eq!(swap_tx.data, "0xswap_calldata_here");
    assert_eq!(swap_tx.value, "0");
}

#[tokio::test]
async fn test_quote_http_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/swap/approval"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": "invalid parameters", "code": "BAD_REQUEST"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let transport = build_transport(&server.uri());
    let result = transport.quote(make_quote_request()).await;

    let err = result.expect_err("quote with 400 should return Transport error");
    let err_msg = format!("{err}");

    // Error should include "transport" and the HTTP status.
    assert!(
        err_msg.contains("transport"),
        "should be a transport error: {err_msg}"
    );
    assert!(
        err_msg.contains("400"),
        "should include HTTP status code: {err_msg}"
    );
    assert!(
        err_msg.contains("invalid parameters"),
        "should include API error body: {err_msg}"
    );
}

#[tokio::test]
async fn test_quote_with_bearer_auth() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/swap/approval"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(swap_approval_response()))
        .expect(1)
        .named("auth-check")
        .mount(&server)
        .await;

    let transport = build_transport(&server.uri());
    let quote = transport
        .quote(make_quote_request())
        .await
        .expect("quote with bearer auth should succeed");

    // If the header didn't match, wiremock would return 404 and the test would fail above.
    assert_eq!(quote.route_id, "across-1-42161");
}

#[tokio::test]
async fn test_quote_without_auth_header() {
    let server = MockServer::start().await;

    // Mount a mock that does NOT require auth header.
    Mock::given(method("GET"))
        .and(path("/swap/approval"))
        .respond_with(ResponseTemplate::new(200).set_body_json(swap_approval_response()))
        .expect(1)
        .mount(&server)
        .await;

    let transport = build_transport_no_auth(&server.uri());
    let quote = transport
        .quote(make_quote_request())
        .await
        .expect("quote without auth should succeed");

    assert_eq!(quote.route_id, "across-1-42161");
}

// ---------------------------------------------------------------------------
// sync_status tests
// ---------------------------------------------------------------------------

fn deposit_status_filled_response() -> Value {
    json!({
        "status": "filled",
        "fillTxnRef": "0xfill_tx_hash_abc",
        "depositTxnRef": "0xdeposit_ref_123",
        "originChainId": 1,
        "depositId": 42
    })
}

fn deposit_status_pending_response() -> Value {
    json!({
        "status": "pending",
        "depositTxnRef": "0xdeposit_ref_123",
        "originChainId": 1,
        "depositId": 42
    })
}

#[tokio::test]
async fn test_sync_status_filled() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/deposit/status"))
        .and(query_param("depositTxnRef", "0xdeposit_ref_123"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(deposit_status_filled_response()))
        .expect(1)
        .mount(&server)
        .await;

    let transport = build_transport(&server.uri());
    let status = transport
        .sync_status("0xdeposit_ref_123")
        .await
        .expect("sync_status should succeed");

    assert_eq!(status.status, "filled");
    assert_eq!(status.deposit_id, "0xdeposit_ref_123");
    assert_eq!(status.fill_tx_hash.as_deref(), Some("0xfill_tx_hash_abc"));
    assert_eq!(
        status.destination_tx_id.as_deref(),
        Some("0xfill_tx_hash_abc")
    );
    assert_eq!(status.bridge_fee_usd, 0);
}

#[tokio::test]
async fn test_sync_status_pending() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/deposit/status"))
        .and(query_param("depositTxnRef", "0xdeposit_ref_123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(deposit_status_pending_response()))
        .expect(1)
        .mount(&server)
        .await;

    let transport = build_transport(&server.uri());
    let status = transport
        .sync_status("0xdeposit_ref_123")
        .await
        .expect("sync_status should succeed for pending");

    assert_eq!(status.status, "pending");
    assert_eq!(status.fill_tx_hash, None);
    assert_eq!(status.destination_tx_id, None);
}

// ---------------------------------------------------------------------------
// Full adapter round-trip test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_full_adapter_round_trip() {
    let server = MockServer::start().await;

    // Mount mock for quote (GET /swap/approval)
    Mock::given(method("GET"))
        .and(path("/swap/approval"))
        .respond_with(ResponseTemplate::new(200).set_body_json(swap_approval_response()))
        .expect(1)
        .mount(&server)
        .await;

    // Mount mock for status (GET /deposit/status)
    // The adapter generates deposit_id = "across-deposit-1" (nonce seeded at 0).
    Mock::given(method("GET"))
        .and(path("/deposit/status"))
        .and(query_param("depositTxnRef", "across-deposit-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(deposit_status_filled_response()))
        .expect(1)
        .mount(&server)
        .await;

    let transport = Arc::new(AcrossHttpTransport::new(
        server.uri(),
        Some("test-integrator".into()),
        Some("test-key".into()),
    ));
    let adapter = AcrossAdapter::with_transport(transport, 0);

    let request = make_quote_request();
    let (ack, status) = adapter
        .bridge_asset("0xSigner", "0xRecipient", request)
        .await
        .expect("bridge_asset round-trip should succeed");

    // Verify ack
    assert_eq!(ack.deposit_id, "across-deposit-1");
    assert_eq!(ack.status, "pending");
    assert_eq!(ack.calldata.as_deref(), Some("0xswap_calldata_here"));
    assert!(ack.swap_tx.is_some());

    // Verify status
    assert_eq!(status.status, "filled");
    assert_eq!(status.deposit_id, "across-deposit-1");
    assert_eq!(status.fill_tx_hash.as_deref(), Some("0xfill_tx_hash_abc"));
    assert_eq!(
        status.destination_tx_id.as_deref(),
        Some("0xfill_tx_hash_abc")
    );
}
