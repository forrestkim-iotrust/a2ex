//! HTTP transport for the Hyperliquid exchange and info API endpoints.
//!
//! [`HyperliquidHttpTransport`] signs L1 actions via a [`SignerBridge`] and
//! communicates with the real Hyperliquid REST API:
//! - `POST /exchange` — authenticated order placement, modification, cancellation
//! - `POST /info` — unauthenticated account/order queries

use std::sync::Arc;
use std::time::Duration;

use a2ex_signer_bridge::SignerBridge;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};

use crate::{
    HyperliquidAdapterError, HyperliquidCancelAck, HyperliquidCancelRequest,
    HyperliquidClearinghouseState, HyperliquidExchangeRequest, HyperliquidExchangeResponse,
    HyperliquidInfoRequest, HyperliquidInfoResponse, HyperliquidModifyRequest,
    HyperliquidOpenOrder, HyperliquidOrderAck, HyperliquidOrderStatus, HyperliquidPlaceRequest,
    HyperliquidPlacedOrder, HyperliquidPosition, HyperliquidTransport, HyperliquidUserFill,
    signing,
};

/// HTTP transport that signs exchange actions and communicates with Hyperliquid
/// REST endpoints.
pub struct HyperliquidHttpTransport {
    base_url: String,
    signer: Arc<dyn SignerBridge>,
    client: Client,
    is_mainnet: bool,
}

impl HyperliquidHttpTransport {
    /// Create a new transport.
    ///
    /// Builds a [`reqwest::Client`] with a 5-second timeout.
    pub fn new(
        base_url: impl Into<String>,
        signer: Arc<dyn SignerBridge>,
        is_mainnet: bool,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client builds");
        Self {
            base_url: base_url.into(),
            signer,
            client,
            is_mainnet,
        }
    }
}

// ---------------------------------------------------------------------------
// Action JSON builders — map our domain types to the Hyperliquid wire format
// ---------------------------------------------------------------------------

/// Build the action JSON and action_type string for a place-order request.
fn build_place_action(req: &HyperliquidPlaceRequest) -> (Value, &'static str) {
    let orders: Vec<Value> = req.orders.iter().map(|o| order_to_wire_json(o)).collect();

    let action = json!({
        "type": "order",
        "orders": orders,
        "grouping": "na",
    });
    (action, "order")
}

/// Build the action JSON and action_type string for a modify-order request.
fn build_modify_action(req: &HyperliquidModifyRequest) -> (Value, &'static str) {
    let modifies: Vec<Value> = req
        .modifies
        .iter()
        .map(|m| {
            let order_wire = json!({
                "a": m.asset,
                "b": m.is_buy,
                "p": m.price,
                "s": m.size,
                "r": m.reduce_only,
                "t": { "limit": { "tif": m.time_in_force } },
                "c": m.client_order_id,
            });
            json!({
                "oid": m.order_id,
                "order": order_wire,
            })
        })
        .collect();

    let action = json!({
        "type": "batchModify",
        "modifies": modifies,
    });
    (action, "batchModify")
}

/// Build the action JSON and action_type string for a cancel request.
fn build_cancel_action(req: &HyperliquidCancelRequest) -> (Value, &'static str) {
    let cancels: Vec<Value> = req
        .cancels
        .iter()
        .map(|c| {
            json!({
                "a": 0u32, // asset not in CancelledOrder — default to 0
                "o": c.order_id,
            })
        })
        .collect();

    let action = json!({
        "type": "cancel",
        "cancels": cancels,
    });
    (action, "cancel")
}

/// Convert a placed order to the wire-format JSON object.
fn order_to_wire_json(o: &HyperliquidPlacedOrder) -> Value {
    // Hyperliquid cloid is 16 bytes (32 hex chars) — must not be null
    let cloid = o.client_order_id.clone().unwrap_or_else(|| {
        "0x00000000000000000000000000000000".to_string()
    });
    json!({
        "a": o.asset,
        "b": o.is_buy,
        "p": o.price,
        "s": o.size,
        "r": o.reduce_only,
        "t": { "limit": { "tif": o.time_in_force } },
        "c": cloid,
    })
}

// ---------------------------------------------------------------------------
// Exchange response parsing
// ---------------------------------------------------------------------------

/// Parse a Hyperliquid exchange response body.
///
/// The exchange returns `{"status":"ok","response":{"type":"order"|"cancel","data":{"statuses":[...]}}}`.
/// Errors can appear as:
/// - Top-level `"status"` != `"ok"`
/// - Individual entries in `statuses` that are plain strings (error text)
fn parse_exchange_response(
    body: &Value,
    signer_address: &str,
    account_address: &str,
    nonce: u64,
    is_cancel: bool,
) -> Result<HyperliquidExchangeResponse, HyperliquidAdapterError> {
    // Check top-level status
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "ok" {
        let err_msg = body
            .get("response")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown exchange error");
        return Err(HyperliquidAdapterError::transport(format!(
            "exchange rejected: {err_msg}"
        )));
    }

    let response_obj = body.get("response").ok_or_else(|| {
        HyperliquidAdapterError::transport("exchange response missing 'response' field")
    })?;

    let data = response_obj.get("data").ok_or_else(|| {
        HyperliquidAdapterError::transport("exchange response missing 'data' field")
    })?;

    let statuses = data
        .get("statuses")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            HyperliquidAdapterError::transport("exchange response missing 'statuses' array")
        })?;

    // Check for error entries in statuses.
    // Cancel responses use "success" as a plain string — that is not an error.
    for entry in statuses {
        if let Some(err_str) = entry.as_str() {
            if err_str == "success" {
                continue;
            }
            tracing::warn!(
                error_text = err_str,
                "exchange returned error in statuses array"
            );
            return Err(HyperliquidAdapterError::transport(format!(
                "exchange order error: {err_str}"
            )));
        }
    }

    if is_cancel {
        let order_id = statuses
            .first()
            .and_then(|s| s.get("oid"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Ok(HyperliquidExchangeResponse::Cancel(HyperliquidCancelAck {
            signer_address: signer_address.to_string(),
            account_address: account_address.to_string(),
            nonce,
            status: "ok".to_string(),
            order_id,
        }))
    } else {
        // Extract order_id from first status entry (resting or filled)
        let first_status = statuses.first();
        let order_id = first_status.and_then(|s| {
            s.get("resting")
                .and_then(|r| r.get("oid"))
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    s.get("filled")
                        .and_then(|f| f.get("oid"))
                        .and_then(|v| v.as_u64())
                })
        });

        Ok(HyperliquidExchangeResponse::Order(HyperliquidOrderAck {
            signer_address: signer_address.to_string(),
            account_address: account_address.to_string(),
            nonce,
            status: "ok".to_string(),
            order_id,
            client_order_id: None,
        }))
    }
}

// ---------------------------------------------------------------------------
// Info response parsing
// ---------------------------------------------------------------------------

fn parse_open_orders(body: &Value) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError> {
    let arr = body
        .as_array()
        .ok_or_else(|| HyperliquidAdapterError::transport("openOrders response is not an array"))?;

    let orders: Vec<HyperliquidOpenOrder> = arr
        .iter()
        .map(|o| {
            Ok(HyperliquidOpenOrder {
                order_id: o.get("oid").and_then(|v| v.as_u64()).unwrap_or(0),
                asset: o.get("asset").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                instrument: o
                    .get("coin")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                is_buy: o
                    .get("side")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "B")
                    .unwrap_or(false),
                price: o
                    .get("limitPx")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string(),
                size: o
                    .get("sz")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string(),
                reduce_only: o
                    .get("reduceOnly")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                status: o
                    .get("orderType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("open")
                    .to_string(),
                client_order_id: o
                    .get("cloid")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            })
        })
        .collect::<Result<_, HyperliquidAdapterError>>()?;

    Ok(HyperliquidInfoResponse::OpenOrders(orders))
}

fn parse_order_status(body: &Value) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError> {
    // The orderStatus response wraps in an "order" object with "status" and "order" fields
    let order_obj = body.get("order").unwrap_or(body);

    let order_inner = order_obj.get("order").unwrap_or(order_obj);

    let order_id = order_inner.get("oid").and_then(|v| v.as_u64()).unwrap_or(0);

    let status = order_obj
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let filled_size = order_inner
        .get("filledSz")
        .or_else(|| order_obj.get("filledSz"))
        .and_then(|v| v.as_str())
        .unwrap_or("0")
        .to_string();

    Ok(HyperliquidInfoResponse::OrderStatus(
        HyperliquidOrderStatus {
            order_id,
            status,
            filled_size,
        },
    ))
}

fn parse_user_fills(body: &Value) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError> {
    let arr = body
        .as_array()
        .ok_or_else(|| HyperliquidAdapterError::transport("userFills response is not an array"))?;

    let fills: Vec<HyperliquidUserFill> = arr
        .iter()
        .map(|f| HyperliquidUserFill {
            order_id: f.get("oid").and_then(|v| v.as_u64()).unwrap_or(0),
            asset: f.get("asset").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            instrument: f
                .get("coin")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            size: f
                .get("sz")
                .and_then(|v| v.as_str())
                .unwrap_or("0")
                .to_string(),
            price: f
                .get("px")
                .and_then(|v| v.as_str())
                .unwrap_or("0")
                .to_string(),
            side: f
                .get("side")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            filled_at: f
                .get("time")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect();

    Ok(HyperliquidInfoResponse::UserFills(fills))
}

fn parse_clearinghouse_state(
    body: &Value,
) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError> {
    let asset_positions = body
        .get("assetPositions")
        .and_then(|v| v.as_array())
        .unwrap_or(&Vec::new())
        .clone();

    let positions: Vec<HyperliquidPosition> = asset_positions
        .iter()
        .filter_map(|ap| {
            let pos = ap.get("position")?;
            Some(HyperliquidPosition {
                asset: pos.get("asset").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                instrument: pos
                    .get("coin")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                size: pos
                    .get("szi")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string(),
                entry_price: pos
                    .get("entryPx")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string(),
                position_value: pos
                    .get("positionValue")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string(),
            })
        })
        .collect();

    Ok(HyperliquidInfoResponse::ClearinghouseState(
        HyperliquidClearinghouseState { positions },
    ))
}

// ---------------------------------------------------------------------------
// HyperliquidTransport implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl HyperliquidTransport for HyperliquidHttpTransport {
    async fn submit_exchange(
        &self,
        request: HyperliquidExchangeRequest,
    ) -> Result<HyperliquidExchangeResponse, HyperliquidAdapterError> {
        let (action, action_type, nonce, signer_address, account_address, is_cancel) =
            match &request {
                HyperliquidExchangeRequest::Place(req) => {
                    let (action, at) = build_place_action(req);
                    (
                        action,
                        at,
                        req.nonce,
                        req.signer_address.clone(),
                        req.account_address.clone(),
                        false,
                    )
                }
                HyperliquidExchangeRequest::Modify(req) => {
                    let (action, at) = build_modify_action(req);
                    (
                        action,
                        at,
                        req.nonce,
                        req.signer_address.clone(),
                        req.account_address.clone(),
                        false,
                    )
                }
                HyperliquidExchangeRequest::Cancel(req) => {
                    let (action, at) = build_cancel_action(req);
                    (
                        action,
                        at,
                        req.nonce,
                        req.signer_address.clone(),
                        req.account_address.clone(),
                        true,
                    )
                }
            };

        tracing::info!(
            action_type = action_type,
            nonce = nonce,
            "submitting exchange action"
        );

        // Sign the action
        let signature = signing::sign_l1_action(
            self.signer.as_ref(),
            &action,
            action_type,
            None, // vault_address
            nonce,
            self.is_mainnet,
        )
        .await?;

        // Parse hex signature into {r, s, v} object expected by Hyperliquid
        let sig_hex = signature.strip_prefix("0x").unwrap_or(&signature);
        let sig_bytes = hex::decode(sig_hex).map_err(|e| {
            HyperliquidAdapterError::transport(format!("invalid signature hex: {e}"))
        })?;
        let sig_obj = if sig_bytes.len() == 65 {
            // Normalize v to 27/28 range (Hyperliquid expects legacy recovery id)
            let v_raw = sig_bytes[64];
            let v = if v_raw < 27 { v_raw as u64 + 27 } else { v_raw as u64 };
            json!({
                "r": format!("0x{}", hex::encode(&sig_bytes[..32])),
                "s": format!("0x{}", hex::encode(&sig_bytes[32..64])),
                "v": v,
            })
        } else {
            // Fallback: send as raw hex string
            json!(signature)
        };

        // Build the exchange request body
        // Note: vaultAddress must be absent (not null) when not using vault
        let mut body = json!({
            "action": action,
            "nonce": nonce,
            "signature": sig_obj,
        });
        // Only include vaultAddress if it's set
        body.as_object_mut().unwrap().insert("vaultAddress".to_string(), Value::Null);

        let url = format!("{}/exchange", self.base_url.trim_end_matches('/'));

        tracing::debug!(body = %serde_json::to_string(&body).unwrap_or_default(), "exchange request body");

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "exchange HTTP request failed");
                HyperliquidAdapterError::transport(format!("exchange network error: {e}"))
            })?;

        let status_code = response.status();
        let response_text = response.text().await.map_err(|e| {
            HyperliquidAdapterError::transport(format!("failed to read exchange response: {e}"))
        })?;

        if !status_code.is_success() {
            tracing::warn!(
                status = status_code.as_u16(),
                body = %response_text,
                "exchange returned HTTP error"
            );
            return Err(HyperliquidAdapterError::transport(format!(
                "exchange HTTP {}: {}",
                status_code.as_u16(),
                response_text
            )));
        }

        let response_json: Value = serde_json::from_str(&response_text).map_err(|e| {
            HyperliquidAdapterError::transport(format!(
                "failed to parse exchange response JSON: {e}"
            ))
        })?;

        parse_exchange_response(
            &response_json,
            &signer_address,
            &account_address,
            nonce,
            is_cancel,
        )
    }

    async fn query_info(
        &self,
        request: HyperliquidInfoRequest,
    ) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError> {
        let (body, request_type) = match &request {
            HyperliquidInfoRequest::OpenOrders { account_address } => (
                json!({"type": "openOrders", "user": account_address}),
                "openOrders",
            ),
            HyperliquidInfoRequest::OrderStatus {
                account_address,
                order_id,
            } => (
                json!({"type": "orderStatus", "user": account_address, "oid": order_id}),
                "orderStatus",
            ),
            HyperliquidInfoRequest::UserFills {
                account_address,
                aggregate_by_time,
            } => {
                if *aggregate_by_time {
                    (
                        json!({"type": "userFillsByTime", "user": account_address, "aggregateByTime": true}),
                        "userFillsByTime",
                    )
                } else {
                    (
                        json!({"type": "userFills", "user": account_address}),
                        "userFills",
                    )
                }
            }
            HyperliquidInfoRequest::ClearinghouseState { account_address } => (
                json!({"type": "clearinghouseState", "user": account_address}),
                "clearinghouseState",
            ),
        };

        tracing::debug!(request_type = request_type, "querying info endpoint");

        let url = format!("{}/info", self.base_url.trim_end_matches('/'));

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "info HTTP request failed");
                HyperliquidAdapterError::transport(format!("info network error: {e}"))
            })?;

        let status_code = response.status();
        let response_text = response.text().await.map_err(|e| {
            HyperliquidAdapterError::transport(format!("failed to read info response: {e}"))
        })?;

        if !status_code.is_success() {
            tracing::warn!(
                status = status_code.as_u16(),
                body = %response_text,
                "info endpoint returned HTTP error"
            );
            return Err(HyperliquidAdapterError::transport(format!(
                "info HTTP {}: {}",
                status_code.as_u16(),
                response_text
            )));
        }

        let response_json: Value = serde_json::from_str(&response_text).map_err(|e| {
            HyperliquidAdapterError::transport(format!("failed to parse info response JSON: {e}"))
        })?;

        match &request {
            HyperliquidInfoRequest::OpenOrders { .. } => parse_open_orders(&response_json),
            HyperliquidInfoRequest::OrderStatus { .. } => parse_order_status(&response_json),
            HyperliquidInfoRequest::UserFills { .. } => parse_user_fills(&response_json),
            HyperliquidInfoRequest::ClearinghouseState { .. } => {
                parse_clearinghouse_state(&response_json)
            }
        }
    }

    async fn withdraw(
        &self,
        amount: &str,
        destination: &str,
        _signer_address: &str,
    ) -> Result<String, HyperliquidAdapterError> {
        use a2ex_signer_bridge::{Eip712Domain, TypedDataSignRequest};

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let hl_chain = if self.is_mainnet { "Mainnet" } else { "Testnet" };
        let sig_chain_id = if self.is_mainnet { "0xa4b1" } else { "0x66eee" };
        let chain_id: u64 = if self.is_mainnet { 42161 } else { 421614 };

        // Action includes hyperliquidChain + signatureChainId (required by exchange API)
        let action = json!({
            "type": "withdraw3",
            "hyperliquidChain": hl_chain,
            "signatureChainId": sig_chain_id,
            "destination": destination,
            "amount": amount,
            "time": timestamp,
        });

        let domain = Eip712Domain {
            name: Some("HyperliquidSignTransaction".to_string()),
            version: Some("1".to_string()),
            chain_id: Some(chain_id),
            verifying_contract: Some("0x0000000000000000000000000000000000000000".to_string()),
        };

        let types = json!({
            "HyperliquidTransaction:Withdraw": [
                {"name": "hyperliquidChain", "type": "string"},
                {"name": "destination", "type": "string"},
                {"name": "amount", "type": "string"},
                {"name": "time", "type": "uint64"}
            ],
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"}
            ]
        });

        let message = json!({
            "hyperliquidChain": hl_chain,
            "destination": destination,
            "amount": amount,
            "time": timestamp,
        });

        let sign_request = TypedDataSignRequest {
            payload: vec![], // not used for user-signed actions
            domain: Some(domain),
            types: Some(types),
            primary_type: Some("HyperliquidTransaction:Withdraw".to_string()),
            message: Some(message),
        };

        let signed = self.signer.sign_typed_data(sign_request).await
            .map_err(|e| HyperliquidAdapterError::transport(format!("withdraw signing error: {e}")))?;

        let signature = signed.signature_hex
            .ok_or_else(|| HyperliquidAdapterError::transport("signer returned no hex signature"))?;

        let sig_hex = signature.strip_prefix("0x").unwrap_or(&signature);
        let sig_bytes = hex::decode(sig_hex).map_err(|e| {
            HyperliquidAdapterError::transport(format!("invalid signature hex: {e}"))
        })?;
        let sig_obj = if sig_bytes.len() == 65 {
            let v_raw = sig_bytes[64];
            let v = if v_raw < 27 { v_raw as u64 + 27 } else { v_raw as u64 };
            json!({
                "r": format!("0x{}", hex::encode(&sig_bytes[..32])),
                "s": format!("0x{}", hex::encode(&sig_bytes[32..64])),
                "v": v,
            })
        } else {
            json!(signature)
        };

        let body = json!({
            "action": action,
            "nonce": timestamp,
            "signature": sig_obj,
            "vaultAddress": Value::Null,
        });

        let url = format!("{}/exchange", self.base_url.trim_end_matches('/'));
        let response = self.client.post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| HyperliquidAdapterError::transport(format!("withdraw network error: {e}")))?;

        let status_code = response.status();
        let text = response.text().await.map_err(|e| {
            HyperliquidAdapterError::transport(format!("withdraw response read error: {e}"))
        })?;

        if !status_code.is_success() {
            return Err(HyperliquidAdapterError::transport(format!("withdraw HTTP {status_code}: {text}")));
        }

        Ok(text)
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
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A mock signer that returns a deterministic signature.
    struct MockSigner;

    #[async_trait]
    impl SignerBridge for MockSigner {
        async fn sign_typed_data(
            &self,
            _req: TypedDataSignRequest,
        ) -> Result<SignedPayload, SignerBridgeError> {
            Ok(SignedPayload::with_hex(
                "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef1c".to_string(),
            ))
        }
    }

    fn make_transport(base_url: &str) -> HyperliquidHttpTransport {
        HyperliquidHttpTransport::new(base_url, Arc::new(MockSigner), false)
    }

    // -----------------------------------------------------------------------
    // Exchange: successful order placement
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exchange_place_order_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/exchange"))
            .and(body_partial_json(json!({
                "action": {
                    "type": "order",
                    "grouping": "na",
                },
                "vaultAddress": null,
            })))
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
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let req = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: "0xsigner".to_string(),
            account_address: "0xaccount".to_string(),
            nonce: 1000,
            orders: vec![HyperliquidPlacedOrder {
                asset: 4,
                is_buy: true,
                price: "30000.0".to_string(),
                size: "0.1".to_string(),
                reduce_only: false,
                client_order_id: None,
                time_in_force: "Gtc".to_string(),
            }],
        });

        let result = transport.submit_exchange(req).await;
        let resp = result.expect("should succeed");

        match resp {
            HyperliquidExchangeResponse::Order(ack) => {
                assert_eq!(ack.signer_address, "0xsigner");
                assert_eq!(ack.account_address, "0xaccount");
                assert_eq!(ack.nonce, 1000);
                assert_eq!(ack.status, "ok");
                assert_eq!(ack.order_id, Some(12345));
            }
            other => panic!("expected Order, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Exchange: request body shape verification
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exchange_request_body_shape() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "ok",
                "response": {
                    "type": "order",
                    "data": { "statuses": [{ "resting": { "oid": 1 } }] }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let req = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: "0xsigner".to_string(),
            account_address: "0xaccount".to_string(),
            nonce: 42,
            orders: vec![HyperliquidPlacedOrder {
                asset: 4,
                is_buy: true,
                price: "30000.0".to_string(),
                size: "0.1".to_string(),
                reduce_only: false,
                client_order_id: Some("my-cloid".to_string()),
                time_in_force: "Gtc".to_string(),
            }],
        });

        transport.submit_exchange(req).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);

        let body: Value =
            serde_json::from_slice(&requests[0].body).expect("body should be valid JSON");

        // Top-level fields
        assert_eq!(body["nonce"], 42);
        assert!(body["signature"].is_object() || body["signature"].is_string(), "signature must be object or string");
        assert!(body["vaultAddress"].is_null(), "vaultAddress must be null");

        // Action structure
        let action = &body["action"];
        assert_eq!(action["type"], "order");
        assert_eq!(action["grouping"], "na");
        assert!(action["orders"].is_array());

        let order = &action["orders"][0];
        assert_eq!(order["a"], 4);
        assert_eq!(order["b"], true);
        assert_eq!(order["p"], "30000.0");
        assert_eq!(order["s"], "0.1");
        assert_eq!(order["r"], false);
        assert_eq!(order["t"]["limit"]["tif"], "Gtc");
        assert_eq!(order["c"], "my-cloid");
    }

    // -----------------------------------------------------------------------
    // Exchange: 200-OK-with-error in statuses
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exchange_200_with_error_in_statuses() {
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

        let transport = make_transport(&server.uri());
        let req = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: "0xsigner".to_string(),
            account_address: "0xaccount".to_string(),
            nonce: 1000,
            orders: vec![HyperliquidPlacedOrder {
                asset: 4,
                is_buy: true,
                price: "30000.0".to_string(),
                size: "0.1".to_string(),
                reduce_only: false,
                client_order_id: None,
                time_in_force: "Gtc".to_string(),
            }],
        });

        let err = transport
            .submit_exchange(req)
            .await
            .expect_err("should return error for error-in-statuses");

        match err {
            HyperliquidAdapterError::Transport { message } => {
                assert!(
                    message.contains("Insufficient margin"),
                    "error should contain exchange text: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Exchange: top-level status error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exchange_top_level_error_status() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "err",
                "response": "Nonce too old"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let req = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: "0xsigner".to_string(),
            account_address: "0xaccount".to_string(),
            nonce: 1,
            orders: vec![HyperliquidPlacedOrder {
                asset: 0,
                is_buy: true,
                price: "100.0".to_string(),
                size: "1.0".to_string(),
                reduce_only: false,
                client_order_id: None,
                time_in_force: "Gtc".to_string(),
            }],
        });

        let err = transport.submit_exchange(req).await.unwrap_err();
        match err {
            HyperliquidAdapterError::Transport { message } => {
                assert!(
                    message.contains("Nonce too old"),
                    "should contain exchange rejection text: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Exchange: HTTP 4xx/5xx
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exchange_http_500() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/exchange"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let req = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: "0xsigner".to_string(),
            account_address: "0xaccount".to_string(),
            nonce: 1,
            orders: vec![HyperliquidPlacedOrder {
                asset: 0,
                is_buy: true,
                price: "100.0".to_string(),
                size: "1.0".to_string(),
                reduce_only: false,
                client_order_id: None,
                time_in_force: "Gtc".to_string(),
            }],
        });

        let err = transport.submit_exchange(req).await.unwrap_err();
        match err {
            HyperliquidAdapterError::Transport { message } => {
                assert!(
                    message.contains("500"),
                    "should contain HTTP status: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Exchange: cancel success
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exchange_cancel_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/exchange"))
            .and(body_partial_json(json!({
                "action": { "type": "cancel" },
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "ok",
                "response": {
                    "type": "cancel",
                    "data": {
                        "statuses": ["success"]
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let req = HyperliquidExchangeRequest::Cancel(HyperliquidCancelRequest {
            signer_address: "0xsigner".to_string(),
            account_address: "0xaccount".to_string(),
            nonce: 500,
            cancels: vec![crate::HyperliquidCancelledOrder { order_id: 99999 }],
        });

        let resp = transport.submit_exchange(req).await.unwrap();
        match resp {
            HyperliquidExchangeResponse::Cancel(ack) => {
                assert_eq!(ack.nonce, 500);
                assert_eq!(ack.status, "ok");
            }
            other => panic!("expected Cancel, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Exchange: network error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exchange_network_error() {
        // Connect to a port that isn't listening
        let transport = make_transport("http://127.0.0.1:1");
        let req = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: "0xsigner".to_string(),
            account_address: "0xaccount".to_string(),
            nonce: 1,
            orders: vec![HyperliquidPlacedOrder {
                asset: 0,
                is_buy: true,
                price: "100.0".to_string(),
                size: "1.0".to_string(),
                reduce_only: false,
                client_order_id: None,
                time_in_force: "Gtc".to_string(),
            }],
        });

        let err = transport.submit_exchange(req).await.unwrap_err();
        match err {
            HyperliquidAdapterError::Transport { message } => {
                assert!(
                    message.contains("network error"),
                    "should mention network error: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Info: open orders
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_open_orders_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/info"))
            .and(body_partial_json(json!({
                "type": "openOrders",
                "user": "0xabc",
            })))
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
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let resp = transport
            .query_info(HyperliquidInfoRequest::OpenOrders {
                account_address: "0xabc".to_string(),
            })
            .await
            .unwrap();

        match resp {
            HyperliquidInfoResponse::OpenOrders(orders) => {
                assert_eq!(orders.len(), 1);
                assert_eq!(orders[0].order_id, 111);
                assert_eq!(orders[0].instrument, "ETH");
                assert!(orders[0].is_buy);
                assert_eq!(orders[0].price, "3000.0");
                assert_eq!(orders[0].size, "1.5");
                assert_eq!(orders[0].client_order_id.as_deref(), Some("my-order-1"));
            }
            other => panic!("expected OpenOrders, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Info: order status
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_order_status_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/info"))
            .and(body_partial_json(json!({
                "type": "orderStatus",
                "user": "0xabc",
                "oid": 555,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "order": {
                    "order": {
                        "oid": 555,
                        "filledSz": "0.5"
                    },
                    "status": "open"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let resp = transport
            .query_info(HyperliquidInfoRequest::OrderStatus {
                account_address: "0xabc".to_string(),
                order_id: 555,
            })
            .await
            .unwrap();

        match resp {
            HyperliquidInfoResponse::OrderStatus(s) => {
                assert_eq!(s.order_id, 555);
                assert_eq!(s.status, "open");
                assert_eq!(s.filled_size, "0.5");
            }
            other => panic!("expected OrderStatus, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Info: user fills
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_user_fills_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/info"))
            .and(body_partial_json(json!({
                "type": "userFills",
                "user": "0xabc",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "oid": 777,
                    "asset": 4,
                    "coin": "BTC",
                    "sz": "0.01",
                    "px": "60000.0",
                    "side": "B",
                    "time": "2025-01-01T00:00:00Z"
                }
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let resp = transport
            .query_info(HyperliquidInfoRequest::UserFills {
                account_address: "0xabc".to_string(),
                aggregate_by_time: false,
            })
            .await
            .unwrap();

        match resp {
            HyperliquidInfoResponse::UserFills(fills) => {
                assert_eq!(fills.len(), 1);
                assert_eq!(fills[0].order_id, 777);
                assert_eq!(fills[0].instrument, "BTC");
                assert_eq!(fills[0].price, "60000.0");
                assert_eq!(fills[0].side, "B");
            }
            other => panic!("expected UserFills, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Info: clearinghouse state
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_clearinghouse_state_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/info"))
            .and(body_partial_json(json!({
                "type": "clearinghouseState",
                "user": "0xabc",
            })))
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
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let resp = transport
            .query_info(HyperliquidInfoRequest::ClearinghouseState {
                account_address: "0xabc".to_string(),
            })
            .await
            .unwrap();

        match resp {
            HyperliquidInfoResponse::ClearinghouseState(state) => {
                assert_eq!(state.positions.len(), 1);
                assert_eq!(state.positions[0].asset, 4);
                assert_eq!(state.positions[0].instrument, "ETH");
                assert_eq!(state.positions[0].size, "2.5");
                assert_eq!(state.positions[0].entry_price, "3100.0");
                assert_eq!(state.positions[0].position_value, "7750.0");
            }
            other => panic!("expected ClearinghouseState, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Info: HTTP error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_http_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/info"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        let err = transport
            .query_info(HyperliquidInfoRequest::OpenOrders {
                account_address: "0xabc".to_string(),
            })
            .await
            .unwrap_err();

        match err {
            HyperliquidAdapterError::Transport { message } => {
                assert!(message.contains("429"), "should contain status: {message}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Info: network error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_network_error() {
        let transport = make_transport("http://127.0.0.1:1");
        let err = transport
            .query_info(HyperliquidInfoRequest::OpenOrders {
                account_address: "0xabc".to_string(),
            })
            .await
            .unwrap_err();

        match err {
            HyperliquidAdapterError::Transport { message } => {
                assert!(
                    message.contains("network error"),
                    "should mention network: {message}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Info: no auth headers on info requests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_no_auth_headers() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        transport
            .query_info(HyperliquidInfoRequest::OpenOrders {
                account_address: "0xabc".to_string(),
            })
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);

        // Verify no Authorization header
        let auth_header = requests[0].headers.get("Authorization");
        assert!(
            auth_header.is_none(),
            "info requests must not include auth headers"
        );
    }

    // -----------------------------------------------------------------------
    // Info: user fills with aggregate_by_time
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn info_user_fills_aggregate_by_time() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/info"))
            .and(body_partial_json(json!({
                "type": "userFillsByTime",
                "aggregateByTime": true,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let transport = make_transport(&server.uri());
        transport
            .query_info(HyperliquidInfoRequest::UserFills {
                account_address: "0xabc".to_string(),
                aggregate_by_time: true,
            })
            .await
            .unwrap();
    }
}
