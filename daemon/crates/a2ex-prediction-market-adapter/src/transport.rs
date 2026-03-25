//! HTTP transport for the Polymarket CLOB API.
//!
//! [`PolymarketHttpTransport`] signs orders via EIP-712 through a [`SignerBridge`],
//! attaches L2 HMAC-SHA256 authentication headers, and communicates with the
//! Polymarket CLOB REST API:
//! - `POST /order` — authenticated order placement
//! - `GET /data/order/{id}` — authenticated order status query

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use a2ex_signer_bridge::SignerBridge;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use crate::signing::{self, CTF_EXCHANGE_ADDRESS, OrderParams, PolymarketApiCredentials};
use crate::{
    PredictionMarketAdapterError, PredictionMarketTransport, PredictionOrderAck,
    PredictionOrderRequest, PredictionOrderStatus, PredictionVenue,
};

/// HTTP transport that signs Polymarket orders via EIP-712 and authenticates
/// API calls with L2 HMAC-SHA256 headers.
pub struct PolymarketHttpTransport {
    base_url: String,
    signer: Arc<dyn SignerBridge>,
    credentials: PolymarketApiCredentials,
    wallet_address: String,
    client: Client,
}

impl PolymarketHttpTransport {
    /// Create a new transport.
    ///
    /// Builds a [`reqwest::Client`] with a 5-second timeout.
    pub fn new(
        base_url: impl Into<String>,
        signer: Arc<dyn SignerBridge>,
        credentials: PolymarketApiCredentials,
        wallet_address: impl Into<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client builds");
        Self {
            base_url: base_url.into(),
            signer,
            credentials,
            wallet_address: wallet_address.into(),
            client,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current UNIX timestamp in seconds as a string.
fn unix_timestamp_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

/// Derive the Polymarket order side constant: `"0"` = BUY, `"1"` = SELL.
fn side_to_poly(side: &str) -> &'static str {
    match side.to_uppercase().as_str() {
        "BUY" | "B" => "0",
        "SELL" | "S" => "1",
        _ => "0",
    }
}

fn parse_fixed_6(value: &str, field: &str) -> Result<u128, PredictionMarketAdapterError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(PredictionMarketAdapterError::transport(format!(
            "{field} cannot be empty"
        )));
    }

    let (whole, frac) = match trimmed.split_once('.') {
        Some((whole, frac)) => (whole, frac),
        None => (trimmed, ""),
    };

    if whole.starts_with('-') || frac.starts_with('-') {
        return Err(PredictionMarketAdapterError::transport(format!(
            "{field} must be non-negative"
        )));
    }

    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return Err(PredictionMarketAdapterError::transport(format!(
            "{field} must be a decimal string"
        )));
    }

    let whole_value = whole.parse::<u128>().map_err(|e| {
        PredictionMarketAdapterError::transport(format!("invalid {field}: {e}"))
    })?;

    let frac_six = if frac.len() > 6 { &frac[..6] } else { frac };
    let mut frac_buf = frac_six.to_owned();
    while frac_buf.len() < 6 {
        frac_buf.push('0');
    }
    let frac_value = if frac_buf.is_empty() {
        0
    } else {
        frac_buf.parse::<u128>().map_err(|e| {
            PredictionMarketAdapterError::transport(format!("invalid {field}: {e}"))
        })?
    };

    Ok(whole_value.saturating_mul(1_000_000).saturating_add(frac_value))
}

/// Compute maker/taker amounts in 6-decimal base units.
///
/// BUY:
/// - makerAmount = quote USDC
/// - takerAmount = outcome tokens
///
/// SELL:
/// - makerAmount = outcome tokens
/// - takerAmount = quote USDC
fn compute_amounts(
    side: &str,
    size: &str,
    price: &str,
) -> Result<(String, String), PredictionMarketAdapterError> {
    let size_units = parse_fixed_6(size, "size")?;
    let price_units = parse_fixed_6(price, "price")?;

    if size_units == 0 {
        return Err(PredictionMarketAdapterError::transport(
            "size must be greater than zero",
        ));
    }
    if price_units == 0 {
        return Err(PredictionMarketAdapterError::transport(
            "price must be greater than zero",
        ));
    }

    let quote_units = size_units.saturating_mul(price_units) / 1_000_000;
    if quote_units == 0 {
        return Err(PredictionMarketAdapterError::transport(
            "size and price produce zero notional",
        ));
    }

    let (maker_amount, taker_amount) = match side_to_poly(side) {
        "0" => (quote_units, size_units),
        "1" => (size_units, quote_units),
        _ => (quote_units, size_units),
    };

    Ok((maker_amount.to_string(), taker_amount.to_string()))
}

// ---------------------------------------------------------------------------
// PredictionMarketTransport implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PredictionMarketTransport for PolymarketHttpTransport {
    async fn place_order(
        &self,
        request: PredictionOrderRequest,
    ) -> Result<PredictionOrderAck, PredictionMarketAdapterError> {
        let token_id = &request.market;
        let side = side_to_poly(&request.side);
        let (maker_amount, taker_amount) =
            compute_amounts(&request.side, &request.size, &request.price)?;
        let salt = signing::generate_order_salt();

        tracing::info!(
            market = %token_id,
            side = %request.side,
            size = %request.size,
            price = %request.price,
            "placing Polymarket order"
        );

        // (a) Build CTF Exchange Order EIP-712 payload
        let order_params = OrderParams {
            salt: salt.clone(),
            maker: self.wallet_address.clone(),
            signer: self.wallet_address.clone(),
            taker: "0x0000000000000000000000000000000000000000".to_string(),
            token_id: token_id.clone(),
            maker_amount: maker_amount.clone(),
            taker_amount: taker_amount.clone(),
            expiration: "0".to_string(),
            nonce: "0".to_string(),
            fee_rate_bps: request.max_fee_bps.to_string(),
            side: side.to_string(),
            signature_type: "0".to_string(),
        };

        let sign_request = signing::build_order_eip712_request(&order_params, CTF_EXCHANGE_ADDRESS);

        // (b) Sign via signer bridge
        let signed = self
            .signer
            .sign_typed_data(sign_request)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "EIP-712 order signing failed");
                PredictionMarketAdapterError::transport(format!("signing failure: {e}"))
            })?;

        let signature = signed
            .signature_hex
            .unwrap_or_else(|| format!("0x{}", hex::encode(&signed.bytes)));

        // (c) Build order JSON body — field types must match Polymarket CLOB expectations
        let fee_bps_num: u64 = order_params.fee_rate_bps.parse().unwrap_or(0);
        let side_str = if side == "0" { "BUY" } else { "SELL" };

        let salt_num: i64 = salt.parse().unwrap_or(0);
        let sig_type_num: u8 = order_params.signature_type.parse().unwrap_or(0);
        let order_body = serde_json::json!({
            "deferExec": false,
            "order": {
                "salt": salt_num,
                "maker": self.wallet_address,
                "signer": self.wallet_address,
                "taker": "0x0000000000000000000000000000000000000000",
                "tokenId": token_id,
                "makerAmount": maker_amount,
                "takerAmount": taker_amount,
                "expiration": order_params.expiration,   // string "0"
                "nonce": order_params.nonce,             // string "0"
                "feeRateBps": order_params.fee_rate_bps, // string "0"
                "side": side_str,                        // string "BUY"/"SELL"
                "signatureType": sig_type_num,           // number 0
                "signature": signature,
            },
            "owner": self.credentials.api_key,
            "orderType": "GTC",
        });

        let body_str = serde_json::to_string(&order_body).map_err(|e| {
            PredictionMarketAdapterError::transport(format!("JSON serialization: {e}"))
        })?;

        tracing::debug!(body = %serde_json::to_string(&order_body).unwrap_or_default(), "polymarket order body");

        // (d) Generate L2 HMAC headers
        let timestamp = unix_timestamp_secs();
        let request_path = "/order";
        let hmac_headers = signing::build_l2_hmac_headers(
            &self.credentials,
            &self.wallet_address,
            &timestamp,
            "POST",
            request_path,
            &body_str,
        )?;

        tracing::debug!(path = request_path, "built HMAC headers for place_order");

        // (e) POST to CLOB
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), request_path);

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        for (key, value) in &hmac_headers {
            req_builder = req_builder.header(key, value);
        }

        let response = req_builder.body(body_str).send().await.map_err(|e| {
            tracing::warn!(error = %e, "CLOB order HTTP request failed");
            PredictionMarketAdapterError::transport(format!("CLOB network error: {e}"))
        })?;

        let status_code = response.status();
        let response_text = response.text().await.map_err(|e| {
            PredictionMarketAdapterError::transport(format!("failed to read CLOB response: {e}"))
        })?;

        // (f) Parse response
        if !status_code.is_success() {
            tracing::warn!(
                status = status_code.as_u16(),
                body = %truncate_body(&response_text, 200),
                "CLOB returned HTTP error"
            );
            return Err(PredictionMarketAdapterError::transport(format!(
                "CLOB HTTP {}: {}",
                status_code.as_u16(),
                truncate_body(&response_text, 200),
            )));
        }

        let response_json: Value = serde_json::from_str(&response_text).map_err(|e| {
            PredictionMarketAdapterError::transport(format!(
                "failed to parse CLOB response JSON: {e}"
            ))
        })?;

        // Check for CLOB-level error
        if let Some(err_msg) = response_json.get("error").and_then(|v| v.as_str()) {
            tracing::warn!(clob_error = err_msg, "CLOB rejected order");
            return Err(PredictionMarketAdapterError::transport(format!(
                "CLOB rejected: {err_msg}"
            )));
        }

        let order_id = response_json
            .get("orderID")
            .or_else(|| response_json.get("order_id"))
            .or_else(|| response_json.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let status = response_json
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("live")
            .to_string();

        Ok(PredictionOrderAck {
            venue: PredictionVenue::Polymarket,
            order_id,
            status,
            idempotency_key: request.idempotency_key,
        })
    }

    async fn sync_order(
        &self,
        _venue: PredictionVenue,
        order_id: &str,
    ) -> Result<PredictionOrderStatus, PredictionMarketAdapterError> {
        let request_path = format!("/data/order/{order_id}");

        tracing::info!(order_id = order_id, "syncing Polymarket order status");

        // (a) Build L2 HMAC headers for GET request
        let timestamp = unix_timestamp_secs();
        let hmac_headers = signing::build_l2_hmac_headers(
            &self.credentials,
            &self.wallet_address,
            &timestamp,
            "GET",
            &request_path,
            "",
        )?;

        tracing::debug!(
            path = %request_path,
            "built HMAC headers for sync_order"
        );

        // (b) GET order status
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), request_path);

        let mut req_builder = self.client.get(&url);

        for (key, value) in &hmac_headers {
            req_builder = req_builder.header(key, value);
        }

        let response = req_builder.send().await.map_err(|e| {
            tracing::warn!(error = %e, "CLOB order status HTTP request failed");
            PredictionMarketAdapterError::transport(format!("CLOB network error: {e}"))
        })?;

        let status_code = response.status();
        let response_text = response.text().await.map_err(|e| {
            PredictionMarketAdapterError::transport(format!(
                "failed to read CLOB status response: {e}"
            ))
        })?;

        if !status_code.is_success() {
            tracing::warn!(
                status = status_code.as_u16(),
                body = %truncate_body(&response_text, 200),
                "CLOB order status returned HTTP error"
            );
            return Err(PredictionMarketAdapterError::transport(format!(
                "CLOB HTTP {}: {}",
                status_code.as_u16(),
                truncate_body(&response_text, 200),
            )));
        }

        // (c) Parse response
        let response_json: Value = serde_json::from_str(&response_text).map_err(|e| {
            PredictionMarketAdapterError::transport(format!(
                "failed to parse CLOB status response JSON: {e}"
            ))
        })?;

        // Check for CLOB-level error
        if let Some(err_msg) = response_json.get("error").and_then(|v| v.as_str()) {
            tracing::warn!(clob_error = err_msg, "CLOB returned error for order status");
            return Err(PredictionMarketAdapterError::transport(format!(
                "CLOB error: {err_msg}"
            )));
        }

        let status = response_json
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let filled_amount = response_json
            .get("filledSize")
            .or_else(|| response_json.get("filled_size"))
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => "0".to_owned(),
            })
            .unwrap_or_else(|| "0".to_owned());

        Ok(PredictionOrderStatus {
            venue: PredictionVenue::Polymarket,
            order_id: order_id.to_string(),
            status,
            filled_amount,
        })
    }
}

/// Truncate a response body for logging (never log full large payloads).
fn truncate_body(body: &str, max_len: usize) -> &str {
    if body.len() <= max_len {
        body
    } else {
        &body[..max_len]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use a2ex_signer_bridge::{
        SignedPayload, SignerBridge, SignerBridgeError, TypedDataSignRequest,
    };
    use base64::{
        Engine, engine::general_purpose::STANDARD as BASE64,
        engine::general_purpose::URL_SAFE as BASE64_URL_SAFE,
    };
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A mock signer that returns a deterministic hex signature.
    struct MockSigner;

    #[async_trait]
    impl SignerBridge for MockSigner {
        async fn sign_typed_data(
            &self,
            _req: TypedDataSignRequest,
        ) -> Result<SignedPayload, SignerBridgeError> {
            Ok(SignedPayload::with_hex(
                "0xdeadbeef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef12345678901c".to_string(),
            ))
        }
    }

    /// A mock signer that always fails.
    struct FailingSigner;

    #[async_trait]
    impl SignerBridge for FailingSigner {
        async fn sign_typed_data(
            &self,
            _req: TypedDataSignRequest,
        ) -> Result<SignedPayload, SignerBridgeError> {
            Err(SignerBridgeError::PeerValidation {
                reason: "test signer failure".to_string(),
            })
        }
    }

    fn test_credentials() -> PolymarketApiCredentials {
        let secret_raw = b"test-secret-bytes";
        PolymarketApiCredentials {
            api_key: "test-api-key".to_string(),
            secret: BASE64.encode(secret_raw),
            passphrase: "test-passphrase".to_string(),
        }
    }

    fn make_transport(base_url: &str) -> PolymarketHttpTransport {
        PolymarketHttpTransport::new(
            base_url,
            Arc::new(MockSigner),
            test_credentials(),
            "0xWalletAddr",
        )
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
            auth: crate::PredictionAuth {
                credential_id: "cred-1".to_string(),
                auth_summary: "test".to_string(),
            },
        }
    }

    // -----------------------------------------------------------------------
    // place_order: correct JSON body shape with order fields and signature
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn place_order_body_shape() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/order"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "orderID": "order-123",
                "status": "live",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        transport.place_order(test_order_request()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);

        let body: Value =
            serde_json::from_slice(&requests[0].body).expect("body should be valid JSON");

        // Top-level fields
        assert!(body["signature"].is_string(), "signature must be present");
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
        assert!(order["salt"].is_string());
        assert_eq!(order["makerAmount"], "5200000");
        assert_eq!(order["takerAmount"], "10000000");
        assert_eq!(order["expiration"], "0");
        assert_eq!(order["nonce"], "0");
        assert_eq!(order["signatureType"], "0");
    }

    // -----------------------------------------------------------------------
    // place_order: attaches all 5 L2 HMAC headers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn place_order_hmac_headers() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/order"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "orderID": "order-456",
                "status": "live",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        transport.place_order(test_order_request()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);

        let headers = &requests[0].headers;

        // All 5 L2 HMAC headers must be present
        let poly_address = headers.get("POLY_ADDRESS").expect("POLY_ADDRESS missing");
        assert_eq!(poly_address.to_str().unwrap(), "0xWalletAddr");

        let poly_sig = headers
            .get("POLY_SIGNATURE")
            .expect("POLY_SIGNATURE missing");
        assert!(
            !poly_sig.to_str().unwrap().is_empty(),
            "POLY_SIGNATURE must be non-empty"
        );
        // Should be valid base64
        BASE64_URL_SAFE
            .decode(poly_sig.to_str().unwrap())
            .expect("POLY_SIGNATURE must be valid base64");

        let poly_ts = headers
            .get("POLY_TIMESTAMP")
            .expect("POLY_TIMESTAMP missing");
        assert!(
            !poly_ts.to_str().unwrap().is_empty(),
            "POLY_TIMESTAMP must be non-empty"
        );

        let poly_key = headers.get("POLY_API_KEY").expect("POLY_API_KEY missing");
        assert_eq!(poly_key.to_str().unwrap(), "test-api-key");

        let poly_pass = headers
            .get("POLY_PASSPHRASE")
            .expect("POLY_PASSPHRASE missing");
        assert_eq!(poly_pass.to_str().unwrap(), "test-passphrase");
    }

    // -----------------------------------------------------------------------
    // sync_order: uses GET with HMAC headers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sync_order_get_with_hmac_headers() {
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

        let transport = make_transport(&server.uri());
        let result = transport
            .sync_order(PredictionVenue::Polymarket, "order-789")
            .await
            .unwrap();

        assert_eq!(result.order_id, "order-789");
        assert_eq!(result.status, "matched");
        assert_eq!(result.filled_amount, "50");
        assert_eq!(result.venue, PredictionVenue::Polymarket);

        // Verify HMAC headers were sent
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);

        let headers = &requests[0].headers;
        assert!(
            headers.get("POLY_ADDRESS").is_some(),
            "POLY_ADDRESS missing"
        );
        assert!(
            headers.get("POLY_SIGNATURE").is_some(),
            "POLY_SIGNATURE missing"
        );
        assert!(
            headers.get("POLY_TIMESTAMP").is_some(),
            "POLY_TIMESTAMP missing"
        );
        assert!(
            headers.get("POLY_API_KEY").is_some(),
            "POLY_API_KEY missing"
        );
        assert!(
            headers.get("POLY_PASSPHRASE").is_some(),
            "POLY_PASSPHRASE missing"
        );
    }

    // -----------------------------------------------------------------------
    // place_order: CLOB error JSON maps to Transport error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn place_order_clob_error_maps_to_transport() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/order"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": "insufficient balance",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let err = transport
            .place_order(test_order_request())
            .await
            .expect_err("should return error for CLOB rejection");

        match err {
            PredictionMarketAdapterError::Transport { message } => {
                assert!(
                    message.contains("insufficient balance"),
                    "error should contain CLOB text: {message}"
                );
                assert!(
                    message.contains("CLOB rejected"),
                    "error should mention CLOB rejection: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // place_order: HTTP 500 maps to Transport error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn place_order_http_500_maps_to_transport() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/order"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
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
            }
        }
    }

    // -----------------------------------------------------------------------
    // place_order: signer failure propagates as Transport error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn place_order_signer_failure_propagates() {
        let server = MockServer::start().await;

        // Mount a mock but it should never be called
        Mock::given(method("POST"))
            .and(path("/order"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "orderID": "should-not-reach",
            })))
            .expect(0)
            .mount(&server)
            .await;

        let transport = PolymarketHttpTransport::new(
            server.uri(),
            Arc::new(FailingSigner),
            test_credentials(),
            "0xWalletAddr",
        );

        let err = transport
            .place_order(test_order_request())
            .await
            .expect_err("should return error when signer fails");

        match err {
            PredictionMarketAdapterError::Transport { message } => {
                assert!(
                    message.contains("signing failure"),
                    "error should mention signing: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // sync_order: HTTP 500 maps to Transport error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sync_order_http_500() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/data/order/order-bad"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let err = transport
            .sync_order(PredictionVenue::Polymarket, "order-bad")
            .await
            .expect_err("should return error for HTTP 500");

        match err {
            PredictionMarketAdapterError::Transport { message } => {
                assert!(
                    message.contains("500"),
                    "should contain HTTP status: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // sync_order: CLOB error JSON maps to Transport error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sync_order_clob_error() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/data/order/order-err"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": "order not found",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let err = transport
            .sync_order(PredictionVenue::Polymarket, "order-err")
            .await
            .expect_err("should return error for CLOB error");

        match err {
            PredictionMarketAdapterError::Transport { message } => {
                assert!(
                    message.contains("order not found"),
                    "should contain CLOB error text: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // place_order: successful response parsing
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn place_order_success_response() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/order"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "orderID": "poly-order-001",
                "status": "live",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let ack = transport.place_order(test_order_request()).await.unwrap();

        assert_eq!(ack.venue, PredictionVenue::Polymarket);
        assert_eq!(ack.order_id, "poly-order-001");
        assert_eq!(ack.status, "live");
        assert_eq!(ack.idempotency_key, "test-idemp-key");
    }
}
