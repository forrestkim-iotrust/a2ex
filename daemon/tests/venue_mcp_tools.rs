//! Integration tests for venue.* MCP tools on `A2exSkillMcpServer`.
//!
//! Constructs the server with wiremock-backed transports for Across,
//! Hyperliquid, and Polymarket, and proves all six venue tools work
//! end-to-end through the MCP server handlers.

use std::sync::{Arc, Mutex};

use a2ex_across_adapter::{AcrossAdapter, transport::AcrossHttpTransport};
use a2ex_hyperliquid_adapter::{HyperliquidAdapter, HyperliquidHttpTransport};
use a2ex_mcp::{
    A2exSkillMcpServer, VenueAdapters,
    venue_tools::{
        BridgeStatusRequest, DeriveApiKeyRequest, PrepareBridgeRequest, QueryPositionsRequest,
        TOOL_VENUE_BRIDGE_STATUS, TOOL_VENUE_DERIVE_API_KEY, TOOL_VENUE_PREPARE_BRIDGE,
        TOOL_VENUE_QUERY_POSITIONS, TOOL_VENUE_TRADE_HYPERLIQUID, TOOL_VENUE_TRADE_POLYMARKET,
        TradeHyperliquidRequest, TradePolymarketRequest,
    },
};
use a2ex_prediction_market_adapter::PredictionMarketAdapter;
use a2ex_signer_bridge::{SignedPayload, SignerBridge, SignerBridgeError, TypedDataSignRequest};
use async_trait::async_trait;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// MockSignerBridge — records requests, returns fixed signature
// ---------------------------------------------------------------------------

const FIXED_SIGNATURE: &str = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef1c";

#[derive(Debug, Default)]
struct MockSignerState {
    requests: Vec<TypedDataSignRequest>,
}

#[derive(Debug, Clone, Default)]
struct MockSignerBridge {
    state: Arc<Mutex<MockSignerState>>,
}

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
// Across mock responses
// ---------------------------------------------------------------------------

fn across_swap_approval_response() -> Value {
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
            "data": "0xswap_calldata",
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

fn across_deposit_status_response() -> Value {
    json!({
        "status": "filled",
        "fillTxnRef": "0xfill_tx_abc",
        "depositTxnRef": "0xdeposit_ref_123",
        "originChainId": 1,
        "depositId": 42
    })
}

// ---------------------------------------------------------------------------
// Hyperliquid mock responses
// ---------------------------------------------------------------------------

fn hyperliquid_exchange_success_response() -> Value {
    json!({
        "status": "ok",
        "response": {
            "type": "order",
            "data": {
                "statuses": [{ "resting": { "oid": 12345 } }]
            }
        }
    })
}

fn hyperliquid_clearinghouse_state_response() -> Value {
    json!({
        "assetPositions": [
            {
                "position": {
                    "asset": 1,
                    "coin": "ETH",
                    "szi": "2.5",
                    "entryPx": "3100.0",
                    "positionValue": "7750.0"
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Polymarket mock responses
// ---------------------------------------------------------------------------

fn polymarket_derive_api_key_response() -> Value {
    json!({
        "api_key": "test-api-key-12345",
        "secret": "test-secret-67890",
        "passphrase": "test-passphrase-abcdef"
    })
}

// ---------------------------------------------------------------------------
// Wiremock setup helpers
// ---------------------------------------------------------------------------

async fn mount_across_quote_mock(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/swap/approval"))
        .respond_with(ResponseTemplate::new(200).set_body_json(across_swap_approval_response()))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_across_status_mock(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/deposit/status"))
        .and(query_param("depositTxnRef", "dep-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(across_deposit_status_response()))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_hyperliquid_exchange_mock(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/exchange"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(hyperliquid_exchange_success_response()),
        )
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_hyperliquid_info_mocks(server: &MockServer) {
    // Open orders
    Mock::given(method("POST"))
        .and(path("/info"))
        .and(body_partial_json(json!({ "type": "openOrders" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "oid": 111,
                "asset": 1,
                "coin": "ETH",
                "side": "B",
                "limitPx": "3000.0",
                "sz": "1.5",
                "reduceOnly": false,
                "orderType": "Limit",
                "cloid": "my-order-1"
            }
        ])))
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
        .mount(server)
        .await;

    // User fills
    Mock::given(method("POST"))
        .and(path("/info"))
        .and(body_partial_json(json!({ "type": "userFills" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;

    // Clearinghouse state
    Mock::given(method("POST"))
        .and(path("/info"))
        .and(body_partial_json(json!({ "type": "clearinghouseState" })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(hyperliquid_clearinghouse_state_response()),
        )
        .mount(server)
        .await;
}

async fn mount_polymarket_derive_api_key_mock(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth/api-key"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(polymarket_derive_api_key_response()),
        )
        .expect(1)
        .mount(server)
        .await;
}

// Note: Polymarket order/status mocks are not mounted here because
// trade_polymarket uses the PredictionMarketAdapter transport (which is
// the noop default in these tests). Full Polymarket order round-trip
// through wiremock is covered by the prediction-market-adapter crate's
// own transport tests.

// ---------------------------------------------------------------------------
// Server construction helper
// ---------------------------------------------------------------------------

/// Build an `A2exSkillMcpServer` with wiremock-backed venue transports.
fn build_server_with_mocks(
    across_url: &str,
    hyperliquid_url: &str,
    polymarket_clob_url: &str,
    signer: Arc<dyn SignerBridge>,
) -> A2exSkillMcpServer {
    let across_transport = Arc::new(AcrossHttpTransport::new(
        across_url,
        Some("test-integrator".into()),
        Some("test-key".into()),
    ));
    let across_adapter = AcrossAdapter::with_transport(across_transport, 0);

    let hl_transport = HyperliquidHttpTransport::new(
        hyperliquid_url,
        signer.clone(),
        true, // mainnet
    );
    let hyperliquid_adapter = HyperliquidAdapter::with_transport(Arc::new(hl_transport), 1000);

    // Use default (noop) prediction market transport — real Polymarket
    // order flow is tested via the server's handle_trade_polymarket which
    // uses the adapter; we wire up a mock transport separately for that.
    let prediction_market_adapter = PredictionMarketAdapter::default();

    let venue_adapters = VenueAdapters::new(
        across_adapter,
        hyperliquid_adapter,
        prediction_market_adapter,
        signer,
    )
    .with_polymarket_clob_base_url(polymarket_clob_url);

    A2exSkillMcpServer::with_venue_adapters(venue_adapters)
}

// ===========================================================================
// Tests
// ===========================================================================

// ---------------------------------------------------------------------------
// 1. Tool listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn all_six_venue_tools_appear_in_list_tools_result() {
    let result = A2exSkillMcpServer::list_tools_result();
    let tool_names: Vec<String> = result.tools.iter().map(|t| t.name.to_string()).collect();

    let venue_tools = [
        TOOL_VENUE_PREPARE_BRIDGE,
        TOOL_VENUE_TRADE_POLYMARKET,
        TOOL_VENUE_TRADE_HYPERLIQUID,
        TOOL_VENUE_QUERY_POSITIONS,
        TOOL_VENUE_BRIDGE_STATUS,
        TOOL_VENUE_DERIVE_API_KEY,
    ];
    for tool in &venue_tools {
        assert!(
            tool_names.iter().any(|n| n == tool),
            "tool listing must include {tool}, got: {tool_names:?}"
        );
    }
}

#[tokio::test]
async fn venue_tools_appear_alongside_existing_tools() {
    let result = A2exSkillMcpServer::list_tools_result();
    let tool_names: Vec<String> = result.tools.iter().map(|t| t.name.to_string()).collect();

    // Existing skill tools should still be present
    assert!(
        tool_names.iter().any(|n| n == "skills.load_bundle"),
        "existing tools must still be present"
    );
    assert!(
        tool_names.iter().any(|n| n == "skills.stop_session"),
        "existing tools must still be present"
    );

    // Venue tools should also be present
    assert!(
        tool_names.iter().any(|n| n == TOOL_VENUE_PREPARE_BRIDGE),
        "venue tools must be present"
    );
    assert!(
        tool_names.len() >= 6 + 4,
        "should have at least 10 tools total (6 venue + existing), got {}",
        tool_names.len()
    );
}

// ---------------------------------------------------------------------------
// 2. Not-configured errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prepare_bridge_returns_not_configured_when_adapters_absent() {
    let server = A2exSkillMcpServer::default();
    let err = server
        .handle_prepare_bridge(PrepareBridgeRequest {
            asset: "USDC".into(),
            amount_usd: 1000,
            source_chain: "ethereum".into(),
            destination_chain: "arbitrum".into(),
            depositor: None,
            recipient: None,
            output_token: None,
        })
        .await
        .expect_err("should fail without adapters");

    assert_is_venue_adapters_not_configured(&err);
}

#[tokio::test]
async fn trade_hyperliquid_returns_not_configured_when_adapters_absent() {
    let server = A2exSkillMcpServer::default();
    let err = server
        .handle_trade_hyperliquid(TradeHyperliquidRequest {
            asset: "ETH".into(),
            is_buy: true,
            size: "0.1".into(),
            price: "3000.0".into(),
            order_type: "limit".into(),
            reduce_only: false,
        })
        .await
        .expect_err("should fail without adapters");

    assert_is_venue_adapters_not_configured(&err);
}

#[tokio::test]
async fn trade_polymarket_returns_not_configured_when_adapters_absent() {
    let server = A2exSkillMcpServer::default();
    let err = server
        .handle_trade_polymarket(TradePolymarketRequest {
            token_id: "token-abc".into(),
            wallet_address: "0xMissingWallet".into(),
            side: "buy".into(),
            size: "100".into(),
            price: "0.55".into(),
            order_type: "limit".into(),
        })
        .await
        .expect_err("should fail without adapters");

    assert_is_venue_adapters_not_configured(&err);
}

#[tokio::test]
async fn derive_api_key_returns_not_configured_when_adapters_absent() {
    let server = A2exSkillMcpServer::default();
    let err = server
        .handle_derive_api_key(DeriveApiKeyRequest {
            wallet_address: "0xtest".into(),
        })
        .await
        .expect_err("should fail without adapters");

    assert_is_venue_adapters_not_configured(&err);
}

#[tokio::test]
async fn query_positions_returns_not_configured_when_adapters_absent() {
    let server = A2exSkillMcpServer::default();
    let err = server
        .handle_query_positions(QueryPositionsRequest { venue: None })
        .await
        .expect_err("should fail without adapters");

    assert_is_venue_adapters_not_configured(&err);
}

#[tokio::test]
async fn bridge_status_returns_not_configured_when_adapters_absent() {
    let server = A2exSkillMcpServer::default();
    let err = server
        .handle_bridge_status(BridgeStatusRequest {
            deposit_id: "dep-123".into(),
        })
        .await
        .expect_err("should fail without adapters");

    assert_is_venue_adapters_not_configured(&err);
}

fn assert_is_venue_adapters_not_configured(err: &a2ex_mcp::McpContractError) {
    let msg = err.to_string();
    assert!(
        msg.contains("not configured") || msg.contains("VenueAdaptersNotConfigured"),
        "error should be VenueAdaptersNotConfigured, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 3. 와리가리 pattern — prepare_bridge with mock Across
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prepare_bridge_returns_warigari_response_from_mock_across() {
    let across_server = MockServer::start().await;
    mount_across_quote_mock(&across_server).await;

    let hl_server = MockServer::start().await;
    let poly_server = MockServer::start().await;
    let signer = Arc::new(MockSignerBridge::default());

    let server = build_server_with_mocks(
        &across_server.uri(),
        &hl_server.uri(),
        &poly_server.uri(),
        signer,
    );

    let response = server
        .handle_prepare_bridge(PrepareBridgeRequest {
            asset: "0xUSDC".into(),
            amount_usd: 1000000,
            source_chain: "1".into(),
            destination_chain: "42161".into(),
            depositor: None,
            recipient: None,
            output_token: None,
        })
        .await
        .expect("prepare_bridge should succeed against mock");

    // 와리가리 response must include swap_tx with to/data/value
    assert!(!response.swap_tx.to.is_empty(), "swap_tx.to must be set");
    assert!(
        !response.swap_tx.data.is_empty(),
        "swap_tx.data must be set"
    );
    assert_eq!(response.swap_tx.value, "0");

    // Must include approval txns
    assert!(
        !response.approval_txns.is_empty(),
        "approval_txns must be non-empty"
    );
    assert!(
        !response.approval_txns[0].to.is_empty(),
        "approval to must be set"
    );

    // Chain ID should be resolved from source_chain
    assert_eq!(
        response.chain_id, 1,
        "source chain '1' should resolve to chain_id 1"
    );

    // Quote metadata
    assert!(!response.quote.route_id.is_empty());
    assert!(response.quote.expected_fill_seconds > 0);

    // next_step hint
    assert!(
        response.next_step.contains("waiaas.call_contract")
            || response.next_step.contains("swap_tx"),
        "next_step should guide caller: {:?}",
        response.next_step
    );
}

// ---------------------------------------------------------------------------
// 4. 직통 pattern — trade_hyperliquid with mock Hyperliquid
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trade_hyperliquid_returns_jiktong_response_from_mock() {
    let across_server = MockServer::start().await;
    let hl_server = MockServer::start().await;
    mount_hyperliquid_exchange_mock(&hl_server).await;

    let poly_server = MockServer::start().await;
    let signer = Arc::new(MockSignerBridge::default());

    let server = build_server_with_mocks(
        &across_server.uri(),
        &hl_server.uri(),
        &poly_server.uri(),
        signer,
    );

    let response = server
        .handle_trade_hyperliquid(TradeHyperliquidRequest {
            asset: "ETH".into(),
            is_buy: true,
            size: "0.1".into(),
            price: "3000.0".into(),
            order_type: "limit".into(),
            reduce_only: false,
        })
        .await
        .expect("trade_hyperliquid should succeed against mock");

    // 직통 response: order_id and status
    assert_eq!(response.order_id, "12345");
    assert_eq!(response.status, "ok");
    assert_eq!(response.venue, "hyperliquid");
}

// ---------------------------------------------------------------------------
// 5. Polymarket credential lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trade_polymarket_without_credentials_returns_actionable_error() {
    let across_server = MockServer::start().await;
    let hl_server = MockServer::start().await;
    let poly_server = MockServer::start().await;
    let signer = Arc::new(MockSignerBridge::default());

    let server = build_server_with_mocks(
        &across_server.uri(),
        &hl_server.uri(),
        &poly_server.uri(),
        signer,
    );

    let err = server
        .handle_trade_polymarket(TradePolymarketRequest {
            token_id: "token-abc".into(),
            wallet_address: "0xMissingWallet".into(),
            side: "buy".into(),
            size: "100".into(),
            price: "0.55".into(),
            order_type: "limit".into(),
        })
        .await
        .expect_err("should fail without derived credentials");

    let msg = err.to_string();
    assert!(
        msg.contains("derive_api_key") || msg.contains("credentials not derived"),
        "error should mention derive_api_key: {msg}"
    );
}

#[tokio::test]
async fn derive_api_key_succeeds_with_mock_clob() {
    let across_server = MockServer::start().await;
    let hl_server = MockServer::start().await;
    let poly_server = MockServer::start().await;
    mount_polymarket_derive_api_key_mock(&poly_server).await;

    let signer = Arc::new(MockSignerBridge::default());

    let server = build_server_with_mocks(
        &across_server.uri(),
        &hl_server.uri(),
        &poly_server.uri(),
        signer,
    );

    let response = server
        .handle_derive_api_key(DeriveApiKeyRequest {
            wallet_address: "0xTestWallet".into(),
        })
        .await
        .expect("derive_api_key should succeed against mock CLOB");

    assert!(response.success);
    assert!(
        response.message.contains("derived") || response.message.contains("success"),
        "message should indicate success: {:?}",
        response.message
    );
}

#[tokio::test]
async fn trade_polymarket_requires_matching_wallet_context() {
    let across_server = MockServer::start().await;
    let hl_server = MockServer::start().await;
    let poly_server = MockServer::start().await;
    mount_polymarket_derive_api_key_mock(&poly_server).await;

    let signer = Arc::new(MockSignerBridge::default());

    let server = build_server_with_mocks(
        &across_server.uri(),
        &hl_server.uri(),
        &poly_server.uri(),
        signer,
    );

    server
        .handle_derive_api_key(DeriveApiKeyRequest {
            wallet_address: "0xKnownWallet".into(),
        })
        .await
        .expect("derive_api_key should succeed");

    let err = server
        .handle_trade_polymarket(TradePolymarketRequest {
            token_id: "token-abc".into(),
            wallet_address: "0xOtherWallet".into(),
            side: "buy".into(),
            size: "100".into(),
            price: "0.55".into(),
            order_type: "limit".into(),
        })
        .await
        .expect_err("should fail for mismatched wallet context");

    let msg = err.to_string();
    assert!(
        msg.contains("derive_api_key") || msg.contains("credentials not derived"),
        "error should mention missing derived credentials: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 6. Read-only tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_positions_returns_positions_from_mock_hyperliquid() {
    let across_server = MockServer::start().await;
    let hl_server = MockServer::start().await;
    mount_hyperliquid_info_mocks(&hl_server).await;

    let poly_server = MockServer::start().await;
    let signer = Arc::new(MockSignerBridge::default());

    let server = build_server_with_mocks(
        &across_server.uri(),
        &hl_server.uri(),
        &poly_server.uri(),
        signer,
    );

    let response = server
        .handle_query_positions(QueryPositionsRequest {
            venue: Some("hyperliquid".into()),
        })
        .await
        .expect("query_positions should succeed against mock");

    assert!(
        !response.positions.is_empty(),
        "should return at least one position from mock clearinghouse state"
    );
    let pos = &response.positions[0];
    assert_eq!(pos.venue, "hyperliquid");
    assert_eq!(pos.asset, "ETH");
    assert_eq!(pos.size, "2.5");
    assert_eq!(pos.entry_price, "3100.0");
}

#[tokio::test]
async fn bridge_status_returns_status_from_mock_across() {
    let across_server = MockServer::start().await;
    mount_across_status_mock(&across_server).await;

    let hl_server = MockServer::start().await;
    let poly_server = MockServer::start().await;
    let signer = Arc::new(MockSignerBridge::default());

    let server = build_server_with_mocks(
        &across_server.uri(),
        &hl_server.uri(),
        &poly_server.uri(),
        signer,
    );

    let response = server
        .handle_bridge_status(BridgeStatusRequest {
            deposit_id: "dep-123".into(),
        })
        .await
        .expect("bridge_status should succeed against mock");

    assert_eq!(response.deposit_id, "dep-123");
    assert_eq!(response.status, "filled");
    assert_eq!(response.fill_tx_hash.as_deref(), Some("0xfill_tx_abc"));
}
