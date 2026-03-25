//! L1 action signing for Hyperliquid exchange.
//!
//! Converts exchange actions into signed EIP-712 payloads by:
//! 1. Msgpack-serializing the action using wire structs with exact field order
//! 2. Appending vault address (20 bytes) + nonce (8 bytes BE) → keccak256 hash
//! 3. Constructing phantom Agent EIP-712 typed-data request
//! 4. Delegating signature to an external `SignerBridge`

use a2ex_signer_bridge::{Eip712Domain, SignerBridge, TypedDataSignRequest};
use serde::Serialize;
use serde_json::json;
use sha3::{Digest, Keccak256};

use crate::HyperliquidAdapterError;

// ---------------------------------------------------------------------------
// Wire structs — field order MUST match Hyperliquid Python SDK expectations.
// rmp-serde serializes fields in declaration order.
// ---------------------------------------------------------------------------

/// Wire format for a single order within a place-order action.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OrderWire {
    #[serde(rename = "a")]
    pub asset: u32,
    #[serde(rename = "b")]
    pub is_buy: bool,
    #[serde(rename = "p")]
    pub limit_px: String,
    #[serde(rename = "s")]
    pub sz: String,
    #[serde(rename = "r")]
    pub reduce_only: bool,
    #[serde(rename = "t")]
    pub order_type: OrderTypeWire,
    #[serde(rename = "c")]
    pub cloid: Option<String>,
}

/// Wire format for the order type field.
#[derive(Debug, Serialize)]
pub(crate) struct OrderTypeWire {
    pub limit: LimitOrderWire,
}

/// Wire format for limit order parameters.
#[derive(Debug, Serialize)]
pub(crate) struct LimitOrderWire {
    pub tif: String,
}

/// Wire format for a single modify within a modify-order action (batch modify).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModifyWire {
    pub oid: u64,
    pub order: OrderWire,
}

/// Wire format for a single cancel within a cancel action.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CancelWire {
    #[serde(rename = "a")]
    pub asset: u32,
    #[serde(rename = "o")]
    pub oid: u64,
}

// ---------------------------------------------------------------------------
// Action structs — field declaration order = msgpack key order.
// Must match Hyperliquid SDK: type → payload → grouping.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct OrderAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub orders: Vec<OrderWire>,
    pub grouping: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub cancels: Vec<CancelWire>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BatchModifyAction {
    #[serde(rename = "type")]
    pub action_type: String,
    pub modifies: Vec<ModifyWire>,
}

/// Wire format for withdraw3 action.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Withdraw3Action {
    #[serde(rename = "type")]
    pub action_type: String,
    pub hyperliquid_chain: String,
    pub signature_chain_id: String,
    pub amount: String,
    pub time: u64,
    pub destination: String,
}

// ---------------------------------------------------------------------------
// Msgpack serialization
// ---------------------------------------------------------------------------

/// Serialize an exchange action JSON value to msgpack bytes using the
/// appropriate wire struct based on `action_type`.
///
/// Supported action types: `"order"`, `"batchModify"`, `"cancel"`.
pub fn action_to_msgpack(
    action: &serde_json::Value,
    action_type: &str,
) -> Result<Vec<u8>, HyperliquidAdapterError> {
    match action_type {
        "order" => {
            let orders = action
                .get("orders")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    HyperliquidAdapterError::transport("order action missing 'orders' array")
                })?;
            let grouping = action
                .get("grouping")
                .and_then(|v| v.as_str())
                .unwrap_or("na");
            let wire_orders: Vec<OrderWire> = orders
                .iter()
                .map(|o| parse_order_wire(o))
                .collect::<Result<_, _>>()?;

            let action = OrderAction {
                action_type: "order".to_string(),
                orders: wire_orders,
                grouping: grouping.to_string(),
            };
            rmp_serde::to_vec_named(&action)
                .map_err(|e| HyperliquidAdapterError::transport(format!("msgpack order: {e}")))
        }
        "batchModify" => {
            let modifies = action
                .get("modifies")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    HyperliquidAdapterError::transport(
                        "batchModify action missing 'modifies' array",
                    )
                })?;
            let wire_modifies: Vec<ModifyWire> = modifies
                .iter()
                .map(|m| {
                    let oid = m.get("oid").and_then(|v| v.as_u64()).ok_or_else(|| {
                        HyperliquidAdapterError::transport("modify missing 'oid'")
                    })?;
                    let order = m.get("order").ok_or_else(|| {
                        HyperliquidAdapterError::transport("modify missing 'order'")
                    })?;
                    Ok(ModifyWire {
                        oid,
                        order: parse_order_wire(order)?,
                    })
                })
                .collect::<Result<_, HyperliquidAdapterError>>()?;

            let action = BatchModifyAction {
                action_type: "batchModify".to_string(),
                modifies: wire_modifies,
            };
            rmp_serde::to_vec_named(&action)
                .map_err(|e| HyperliquidAdapterError::transport(format!("msgpack modify: {e}")))
        }
        "cancel" => {
            let cancels = action
                .get("cancels")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    HyperliquidAdapterError::transport("cancel action missing 'cancels' array")
                })?;
            let wire_cancels: Vec<CancelWire> = cancels
                .iter()
                .map(|c| {
                    let asset = c.get("a").and_then(|v| v.as_u64()).ok_or_else(|| {
                        HyperliquidAdapterError::transport("cancel missing 'a' (asset)")
                    })? as u32;
                    let oid = c.get("o").and_then(|v| v.as_u64()).ok_or_else(|| {
                        HyperliquidAdapterError::transport("cancel missing 'o' (oid)")
                    })?;
                    Ok(CancelWire { asset, oid })
                })
                .collect::<Result<_, HyperliquidAdapterError>>()?;

            let action = CancelAction {
                action_type: "cancel".to_string(),
                cancels: wire_cancels,
            };
            rmp_serde::to_vec_named(&action)
                .map_err(|e| HyperliquidAdapterError::transport(format!("msgpack cancel: {e}")))
        }
        "withdraw3" => {
            let hyperliquid_chain = action.get("hyperliquidChain").and_then(|v| v.as_str()).unwrap_or("Arbitrum");
            let signature_chain_id = action.get("signatureChainId").and_then(|v| v.as_str()).unwrap_or("0xa4b1");
            let amount = action.get("amount").and_then(|v| v.as_str()).unwrap_or("0");
            let time = action.get("time").and_then(|v| v.as_u64()).unwrap_or(0);
            let destination = action.get("destination").and_then(|v| v.as_str()).unwrap_or("");

            let action = Withdraw3Action {
                action_type: "withdraw3".to_string(),
                hyperliquid_chain: hyperliquid_chain.to_string(),
                signature_chain_id: signature_chain_id.to_string(),
                amount: amount.to_string(),
                time,
                destination: destination.to_string(),
            };
            rmp_serde::to_vec_named(&action)
                .map_err(|e| HyperliquidAdapterError::transport(format!("msgpack withdraw3: {e}")))
        }
        other => Err(HyperliquidAdapterError::transport(format!(
            "unsupported action type for msgpack: {other}"
        ))),
    }
}

fn parse_order_wire(o: &serde_json::Value) -> Result<OrderWire, HyperliquidAdapterError> {
    let asset = o
        .get("a")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| HyperliquidAdapterError::transport("order wire missing 'a' (asset)"))?
        as u32;
    let is_buy = o
        .get("b")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| HyperliquidAdapterError::transport("order wire missing 'b' (is_buy)"))?;
    let limit_px = o
        .get("p")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HyperliquidAdapterError::transport("order wire missing 'p' (limit_px)"))?
        .to_lowercase();
    let sz = o
        .get("s")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HyperliquidAdapterError::transport("order wire missing 's' (sz)"))?
        .to_lowercase();
    let reduce_only = o.get("r").and_then(|v| v.as_bool()).ok_or_else(|| {
        HyperliquidAdapterError::transport("order wire missing 'r' (reduce_only)")
    })?;
    let tif = o
        .get("t")
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.get("tif"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| HyperliquidAdapterError::transport("order wire missing 't.limit.tif'"))?
        .to_string();
    let cloid = o.get("c").and_then(|v| v.as_str()).map(|s| s.to_string());

    Ok(OrderWire {
        asset,
        is_buy,
        limit_px,
        sz,
        reduce_only,
        order_type: OrderTypeWire {
            limit: LimitOrderWire { tif },
        },
        cloid,
    })
}

// ---------------------------------------------------------------------------
// Keccak256 hashing
// ---------------------------------------------------------------------------

/// Hash msgpack bytes + nonce + vault into a 32-byte keccak256 digest.
///
/// Layout (matching Hyperliquid SDK):
/// `msgpack_bytes || nonce (8 bytes BE) || vault_flag (1 byte) || [vault_address (20 bytes)]`
pub fn hash_l1_action(msgpack_bytes: &[u8], vault_address: Option<&str>, nonce: u64) -> [u8; 32] {
    let mut data = Vec::with_capacity(msgpack_bytes.len() + 8 + 1 + 20);
    data.extend_from_slice(msgpack_bytes);
    data.extend_from_slice(&nonce.to_be_bytes());

    match vault_address {
        Some(addr) => {
            data.push(1u8);
            let stripped = addr.strip_prefix("0x").unwrap_or(addr);
            let addr_bytes = hex::decode(stripped.to_lowercase()).unwrap_or_else(|_| vec![0u8; 20]);
            let len = addr_bytes.len().min(20);
            data.extend_from_slice(&addr_bytes[..len]);
        }
        None => {
            data.push(0u8);
        }
    }

    let mut hasher = Keccak256::new();
    hasher.update(&data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

// ---------------------------------------------------------------------------
// EIP-712 phantom Agent construction
// ---------------------------------------------------------------------------

/// Build a `TypedDataSignRequest` for the phantom Agent EIP-712 structure.
///
/// - `source`: `"a"` for mainnet, `"b"` for testnet
/// - `connectionId`: the keccak256 hash as `0x`-prefixed hex (bytes32)
/// - Domain: `{ name: "Exchange", version: "1", chainId: 1337, verifyingContract: 0x000...000 }`
pub fn build_agent_eip712_request(
    connection_id: [u8; 32],
    is_mainnet: bool,
) -> TypedDataSignRequest {
    let source = if is_mainnet { "a" } else { "b" };
    let connection_id_hex = format!("0x{}", hex::encode(connection_id));

    let domain = Eip712Domain {
        name: Some("Exchange".to_string()),
        version: Some("1".to_string()),
        chain_id: Some(1337),
        verifying_contract: Some("0x0000000000000000000000000000000000000000".to_string()),
    };

    let types = json!({
        "Agent": [
            { "name": "source", "type": "string" },
            { "name": "connectionId", "type": "bytes32" }
        ],
        "EIP712Domain": [
            { "name": "name", "type": "string" },
            { "name": "version", "type": "string" },
            { "name": "chainId", "type": "uint256" },
            { "name": "verifyingContract", "type": "address" }
        ]
    });

    let message = json!({
        "source": source,
        "connectionId": connection_id_hex,
    });

    TypedDataSignRequest {
        payload: connection_id.to_vec(),
        domain: Some(domain),
        types: Some(types),
        primary_type: Some("Agent".to_string()),
        message: Some(message),
    }
}

// ---------------------------------------------------------------------------
// Full signing orchestration
// ---------------------------------------------------------------------------

/// Sign an L1 action for Hyperliquid exchange submission.
///
/// Flow: action → msgpack → hash(msgpack + vault + nonce) → build Agent EIP-712 → sign → hex signature
pub async fn sign_l1_action(
    signer: &dyn SignerBridge,
    action: &serde_json::Value,
    action_type: &str,
    vault_address: Option<&str>,
    nonce: u64,
    is_mainnet: bool,
) -> Result<String, HyperliquidAdapterError> {
    // Step 1: Serialize action to msgpack
    let msgpack_bytes = action_to_msgpack(action, action_type)?;
    tracing::debug!(
        action_type = action_type,
        msgpack_hex = hex::encode(&msgpack_bytes),
        "serialized action to msgpack"
    );

    // Step 2: Hash msgpack + nonce + vault
    let connection_id = hash_l1_action(&msgpack_bytes, vault_address, nonce);
    tracing::debug!(
        connection_id_hex = hex::encode(connection_id),
        nonce = nonce,
        "computed L1 action hash"
    );

    // Step 3: Build EIP-712 request
    let eip712_request = build_agent_eip712_request(connection_id, is_mainnet);

    // Step 4: Sign via signer bridge
    let signed = signer
        .sign_typed_data(eip712_request)
        .await
        .map_err(|e| HyperliquidAdapterError::transport(format!("signer bridge error: {e}")))?;

    let signature_hex = signed
        .signature_hex
        .ok_or_else(|| HyperliquidAdapterError::transport("signer returned no hex signature"))?;

    Ok(signature_hex)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn msgpack_order_deterministic() {
        let action = json!({
            "type": "order",
            "orders": [
                {
                    "a": 4,
                    "b": true,
                    "p": "30000.0",
                    "s": "0.1",
                    "r": false,
                    "t": { "limit": { "tif": "Gtc" } },
                    "c": null
                }
            ],
            "grouping": "na"
        });

        let bytes1 = action_to_msgpack(&action, "order").unwrap();
        let bytes2 = action_to_msgpack(&action, "order").unwrap();
        assert_eq!(bytes1, bytes2, "msgpack output must be deterministic");
        assert!(!bytes1.is_empty(), "msgpack output must not be empty");
    }

    #[test]
    fn msgpack_cancel_deterministic() {
        let action = json!({
            "type": "cancel",
            "cancels": [
                { "a": 4, "o": 12345 }
            ]
        });

        let bytes1 = action_to_msgpack(&action, "cancel").unwrap();
        let bytes2 = action_to_msgpack(&action, "cancel").unwrap();
        assert_eq!(bytes1, bytes2);
        assert!(!bytes1.is_empty());
    }

    #[test]
    fn msgpack_batch_modify_deterministic() {
        let action = json!({
            "type": "batchModify",
            "modifies": [
                {
                    "oid": 99999,
                    "order": {
                        "a": 4,
                        "b": false,
                        "p": "29000.0",
                        "s": "0.2",
                        "r": true,
                        "t": { "limit": { "tif": "Ioc" } },
                        "c": "my-cloid"
                    }
                }
            ]
        });

        let bytes1 = action_to_msgpack(&action, "batchModify").unwrap();
        let bytes2 = action_to_msgpack(&action, "batchModify").unwrap();
        assert_eq!(bytes1, bytes2);
        assert!(!bytes1.is_empty());
    }

    #[test]
    fn hash_l1_action_deterministic() {
        let msgpack_bytes = b"test-payload";
        let hash1 = hash_l1_action(msgpack_bytes, None, 1000);
        let hash2 = hash_l1_action(msgpack_bytes, None, 1000);
        assert_eq!(hash1, hash2, "hash must be deterministic");
    }

    #[test]
    fn hash_l1_action_keccak256_correctness() {
        // Verify the hash matches: msgpack || nonce(8B BE) || vault_flag(1B)
        let msgpack_bytes = b"hello";
        let nonce: u64 = 42;
        let hash = hash_l1_action(msgpack_bytes, None, nonce);

        let mut expected_input = Vec::new();
        expected_input.extend_from_slice(b"hello");
        expected_input.extend_from_slice(&42u64.to_be_bytes()); // nonce first
        expected_input.push(0u8); // no vault flag

        let mut hasher = Keccak256::new();
        hasher.update(&expected_input);
        let expected = hasher.finalize();
        let mut expected_arr = [0u8; 32];
        expected_arr.copy_from_slice(&expected);

        assert_eq!(hash, expected_arr);
    }

    #[test]
    fn hash_l1_action_with_vault_address() {
        let msgpack_bytes = b"payload";
        let vault = "0xabcdef0123456789abcdef0123456789abcdef01";

        let hash_with_vault = hash_l1_action(msgpack_bytes, Some(vault), 100);
        let hash_without_vault = hash_l1_action(msgpack_bytes, None, 100);

        assert_ne!(
            hash_with_vault, hash_without_vault,
            "vault address must affect the hash"
        );
    }

    #[test]
    fn hash_l1_action_vault_address_lowercased() {
        let msgpack_bytes = b"payload";
        let vault_upper = "0xABCDEF0123456789ABCDEF0123456789ABCDEF01";
        let vault_lower = "0xabcdef0123456789abcdef0123456789abcdef01";

        let hash1 = hash_l1_action(msgpack_bytes, Some(vault_upper), 100);
        let hash2 = hash_l1_action(msgpack_bytes, Some(vault_lower), 100);

        assert_eq!(hash1, hash2, "vault address casing must not affect hash");
    }

    #[test]
    fn hash_l1_action_different_nonce_changes_hash() {
        let msgpack_bytes = b"payload";
        let hash1 = hash_l1_action(msgpack_bytes, None, 100);
        let hash2 = hash_l1_action(msgpack_bytes, None, 101);
        assert_ne!(
            hash1, hash2,
            "different nonces must produce different hashes"
        );
    }

    #[test]
    fn eip712_request_domain_correctness() {
        let connection_id = [0xab; 32];
        let req = build_agent_eip712_request(connection_id, true);

        let domain = req.domain.as_ref().expect("domain required");
        assert_eq!(domain.name.as_deref(), Some("Exchange"));
        assert_eq!(domain.version.as_deref(), Some("1"));
        assert_eq!(domain.chain_id, Some(1337));
        assert_eq!(
            domain.verifying_contract.as_deref(),
            Some("0x0000000000000000000000000000000000000000")
        );
    }

    #[test]
    fn eip712_request_agent_type_fields() {
        let connection_id = [0x01; 32];
        let req = build_agent_eip712_request(connection_id, true);

        let types = req.types.as_ref().expect("types required");
        let agent_fields = types.get("Agent").expect("Agent type required");
        let fields: Vec<(String, String)> = agent_fields
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().unwrap().to_string(),
                    f["type"].as_str().unwrap().to_string(),
                )
            })
            .collect();

        assert_eq!(
            fields,
            vec![
                ("source".to_string(), "string".to_string()),
                ("connectionId".to_string(), "bytes32".to_string()),
            ]
        );

        assert_eq!(req.primary_type.as_deref(), Some("Agent"));
    }

    #[test]
    fn eip712_request_mainnet_source() {
        let connection_id = [0x00; 32];
        let req = build_agent_eip712_request(connection_id, true);
        let msg = req.message.as_ref().expect("message required");
        assert_eq!(msg["source"].as_str(), Some("a"));
    }

    #[test]
    fn eip712_request_testnet_source() {
        let connection_id = [0x00; 32];
        let req = build_agent_eip712_request(connection_id, false);
        let msg = req.message.as_ref().expect("message required");
        assert_eq!(msg["source"].as_str(), Some("b"));
    }

    #[test]
    fn eip712_request_connection_id_hex_format() {
        let connection_id = [0xde; 32];
        let req = build_agent_eip712_request(connection_id, true);
        let msg = req.message.as_ref().expect("message required");
        let cid = msg["connectionId"].as_str().unwrap();
        assert!(cid.starts_with("0x"), "connectionId must be 0x-prefixed");
        assert_eq!(cid.len(), 66, "connectionId must be 66 chars (0x + 64 hex)");
        assert_eq!(cid, format!("0x{}", "de".repeat(32)));
    }

    #[test]
    fn eip712_request_payload_is_connection_id_bytes() {
        let connection_id = [0x42; 32];
        let req = build_agent_eip712_request(connection_id, true);
        assert_eq!(req.payload, connection_id.to_vec());
    }

    #[test]
    fn address_lowercasing_in_order_fields() {
        // Verify price/size strings get lowercased (addresses embedded in these
        // fields would be lowercased by our parse_order_wire)
        let action = json!({
            "type": "order",
            "orders": [
                {
                    "a": 0,
                    "b": true,
                    "p": "ABC",
                    "s": "DEF",
                    "r": false,
                    "t": { "limit": { "tif": "Gtc" } },
                    "c": null
                }
            ],
            "grouping": "na"
        });

        let bytes = action_to_msgpack(&action, "order").unwrap();
        // The msgpack output should contain lowercase versions
        let hex_output = hex::encode(&bytes);
        // "abc" and "def" should appear (lowercase), not "ABC" and "DEF"
        assert!(
            !hex_output.contains(&hex::encode(b"ABC")),
            "uppercase should be lowercased in order wire"
        );
    }

    #[test]
    fn unsupported_action_type_returns_error() {
        let action = json!({});
        let result = action_to_msgpack(&action, "unknown_type");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("unsupported action type"),
            "error: {err}"
        );
    }
}
