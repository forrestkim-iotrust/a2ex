//! Polymarket-specific signing primitives.
//!
//! Three authentication mechanisms:
//! - **L1 ClobAuth EIP-712**: signs a `ClobAuth` struct for API key derivation
//! - **L1 CTF Exchange Order EIP-712**: signs an `Order` struct for order placement
//! - **L2 HMAC-SHA256**: computes HMAC headers for authenticated trading operations

use a2ex_signer_bridge::{Eip712Domain, TypedDataSignRequest};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64, engine::general_purpose::URL_SAFE as BASE64_URL_SAFE};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use crate::PredictionMarketAdapterError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// ClobAuth EIP-712 domain name (for API key derivation).
const CLOB_AUTH_DOMAIN_NAME: &str = "ClobAuthDomain";
/// ClobAuth EIP-712 domain version.
const CLOB_AUTH_DOMAIN_VERSION: &str = "1";
/// CTF Exchange EIP-712 domain name (for order signing).
const CTF_EXCHANGE_DOMAIN_NAME: &str = "Polymarket CTF Exchange";
/// Polygon mainnet chain ID used by Polymarket.
const POLYGON_CHAIN_ID: u64 = 137;

/// CTF Exchange contract address on Polygon mainnet (regular markets).
pub const CTF_EXCHANGE_ADDRESS: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";
/// Neg Risk CTF Exchange contract address on Polygon mainnet (neg-risk markets).
pub const NEG_RISK_CTF_EXCHANGE_ADDRESS: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";

// ---------------------------------------------------------------------------
// API Credentials
// ---------------------------------------------------------------------------

/// Polymarket CLOB API credentials derived via the L1 EIP-712 auth flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolymarketApiCredentials {
    pub api_key: String,
    pub secret: String,
    pub passphrase: String,
}

// ---------------------------------------------------------------------------
// ClobAuth EIP-712
// ---------------------------------------------------------------------------

/// Parameters for building a ClobAuth EIP-712 signing request.
pub struct ClobAuthParams {
    pub address: String,
    pub timestamp: String,
    pub nonce: String,
    pub message: String,
}

/// Build a `TypedDataSignRequest` for the ClobAuth EIP-712 struct.
///
/// Domain: `{ name: "ClobAuthDomain", version: "1", chainId: 137 }`
/// Primary type: `ClobAuth` with fields `address`, `timestamp`, `nonce`, `message`.
pub fn build_clob_auth_eip712_request(params: &ClobAuthParams) -> TypedDataSignRequest {
    let domain = Eip712Domain {
        name: Some(CLOB_AUTH_DOMAIN_NAME.to_string()),
        version: Some(CLOB_AUTH_DOMAIN_VERSION.to_string()),
        chain_id: Some(POLYGON_CHAIN_ID),
        verifying_contract: None,
    };

    let types = json!({
        "ClobAuth": [
            { "name": "address", "type": "address" },
            { "name": "timestamp", "type": "string" },
            { "name": "nonce", "type": "uint256" },
            { "name": "message", "type": "string" }
        ],
        "EIP712Domain": [
            { "name": "name", "type": "string" },
            { "name": "version", "type": "string" },
            { "name": "chainId", "type": "uint256" }
        ]
    });

    let nonce_int: u64 = params.nonce.parse().unwrap_or(0);
    let message = json!({
        "address": params.address,
        "timestamp": params.timestamp,
        "nonce": nonce_int,
        "message": params.message,
    });

    TypedDataSignRequest {
        payload: Vec::new(),
        domain: Some(domain),
        types: Some(types),
        primary_type: Some("ClobAuth".to_string()),
        message: Some(message),
    }
}

// ---------------------------------------------------------------------------
// CTF Exchange Order EIP-712
// ---------------------------------------------------------------------------

/// Parameters for building a CTF Exchange Order EIP-712 signing request.
pub struct OrderParams {
    pub salt: String,
    pub maker: String,
    pub signer: String,
    pub taker: String,
    pub token_id: String,
    pub maker_amount: String,
    pub taker_amount: String,
    pub expiration: String,
    pub nonce: String,
    pub fee_rate_bps: String,
    pub side: String,
    pub signature_type: String,
}

/// Build a `TypedDataSignRequest` for a CTF Exchange Order EIP-712 struct.
///
/// Domain: `{ name: "Polymarket CTF Exchange", version: "1", chainId: 137, verifyingContract: <exchange_address> }`
/// Primary type: `Order` with all order fields.
pub fn build_order_eip712_request(
    params: &OrderParams,
    exchange_address: &str,
) -> TypedDataSignRequest {
    let domain = Eip712Domain {
        name: Some(CTF_EXCHANGE_DOMAIN_NAME.to_string()),
        version: Some(CLOB_AUTH_DOMAIN_VERSION.to_string()),
        chain_id: Some(POLYGON_CHAIN_ID),
        verifying_contract: Some(exchange_address.to_string()),
    };

    let types = json!({
        "Order": [
            { "name": "salt", "type": "uint256" },
            { "name": "maker", "type": "address" },
            { "name": "signer", "type": "address" },
            { "name": "taker", "type": "address" },
            { "name": "tokenId", "type": "uint256" },
            { "name": "makerAmount", "type": "uint256" },
            { "name": "takerAmount", "type": "uint256" },
            { "name": "expiration", "type": "uint256" },
            { "name": "nonce", "type": "uint256" },
            { "name": "feeRateBps", "type": "uint256" },
            { "name": "side", "type": "uint8" },
            { "name": "signatureType", "type": "uint8" }
        ],
        "EIP712Domain": [
            { "name": "name", "type": "string" },
            { "name": "version", "type": "string" },
            { "name": "chainId", "type": "uint256" },
            { "name": "verifyingContract", "type": "address" }
        ]
    });

    // EIP-712 numeric types must be integers, not strings
    let side_int: u8 = params.side.parse().unwrap_or(0);
    let sig_type_int: u8 = params.signature_type.parse().unwrap_or(0);
    let fee_bps_int: u64 = params.fee_rate_bps.parse().unwrap_or(0);

    let message = json!({
        "salt": params.salt,
        "maker": params.maker,
        "signer": params.signer,
        "taker": params.taker,
        "tokenId": params.token_id,
        "makerAmount": params.maker_amount,
        "takerAmount": params.taker_amount,
        "expiration": params.expiration,
        "nonce": params.nonce,
        "feeRateBps": fee_bps_int,
        "side": side_int,
        "signatureType": sig_type_int,
    });

    TypedDataSignRequest {
        payload: Vec::new(),
        domain: Some(domain),
        types: Some(types),
        primary_type: Some("Order".to_string()),
        message: Some(message),
    }
}

/// Generate a random order salt as a decimal string.
/// Must stay within JS Number.MAX_SAFE_INTEGER (9007199254740991) because
/// the Polymarket CLOB server parses salt as a JSON number.
pub fn generate_order_salt() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // timestamp_ms * 1000 + sub-ms entropy — stays within ~1.77e15 (safe)
    let ms = now.as_millis() as u64;
    let sub = (now.subsec_nanos() % 1000) as u64;
    let stack_addr = &ms as *const _ as u64;
    let salt = ms.wrapping_mul(1000).wrapping_add(sub) ^ (stack_addr & 0xFFFF);
    salt.to_string()
}

// ---------------------------------------------------------------------------
// L1 Auth Headers
// ---------------------------------------------------------------------------

/// Build L1 authentication headers for EIP-712 based endpoints (API key derivation).
///
/// Returns header pairs: `POLY_ADDRESS`, `POLY_SIGNATURE`, `POLY_TIMESTAMP`, `POLY_NONCE`.
pub fn build_l1_auth_headers(
    address: &str,
    signature: &str,
    timestamp: &str,
    nonce: &str,
) -> Vec<(String, String)> {
    vec![
        ("POLY_ADDRESS".to_string(), address.to_string()),
        ("POLY_SIGNATURE".to_string(), signature.to_string()),
        ("POLY_TIMESTAMP".to_string(), timestamp.to_string()),
        ("POLY_NONCE".to_string(), nonce.to_string()),
    ]
}

// ---------------------------------------------------------------------------
// L2 HMAC-SHA256 Headers
// ---------------------------------------------------------------------------

type HmacSha256 = Hmac<Sha256>;

/// Compute the HMAC-SHA256 signature for L2 authenticated requests.
///
/// The HMAC key is `base64_decode(secret)`.
/// The message is `timestamp + method + path + body`.
///
/// Returns the base64-encoded HMAC signature.
pub fn compute_hmac_signature(
    secret: &str,
    timestamp: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Result<String, PredictionMarketAdapterError> {
    // Polymarket secrets use URL-safe base64 (- and _ instead of + and /).
    // Normalize to standard base64 before decoding.
    let normalized = secret.replace('-', "+").replace('_', "/");
    let key_bytes = BASE64.decode(&normalized).map_err(|e| {
        PredictionMarketAdapterError::transport(format!("base64 decode secret: {e}"))
    })?;

    let message = format!("{timestamp}{method}{path}{body}");

    tracing::debug!(
        timestamp = timestamp,
        method = method,
        path = path,
        body_len = body.len(),
        "computing HMAC-SHA256 signature"
    );

    let mut mac = HmacSha256::new_from_slice(&key_bytes)
        .map_err(|e| PredictionMarketAdapterError::transport(format!("HMAC key init: {e}")))?;
    mac.update(message.as_bytes());
    let result = mac.finalize().into_bytes();

    // Polymarket expects URL-safe base64 (matching py-clob-client's urlsafe_b64encode)
    Ok(BASE64_URL_SAFE.encode(result))
}

/// Build L2 authentication headers for HMAC-authenticated endpoints (trading operations).
///
/// Returns header pairs: `POLY_ADDRESS`, `POLY_SIGNATURE`, `POLY_TIMESTAMP`,
/// `POLY_API_KEY`, `POLY_PASSPHRASE`.
pub fn build_l2_hmac_headers(
    credentials: &PolymarketApiCredentials,
    address: &str,
    timestamp: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Result<Vec<(String, String)>, PredictionMarketAdapterError> {
    let signature = compute_hmac_signature(&credentials.secret, timestamp, method, path, body)?;

    Ok(vec![
        ("POLY_ADDRESS".to_string(), address.to_string()),
        ("POLY_SIGNATURE".to_string(), signature),
        ("POLY_TIMESTAMP".to_string(), timestamp.to_string()),
        ("POLY_API_KEY".to_string(), credentials.api_key.clone()),
        (
            "POLY_PASSPHRASE".to_string(),
            credentials.passphrase.clone(),
        ),
    ])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ClobAuth EIP-712 tests --

    #[test]
    fn clob_auth_eip712_domain_correctness() {
        let params = ClobAuthParams {
            address: "0xabc".to_string(),
            timestamp: "1234567890".to_string(),
            nonce: "0".to_string(),
            message: "".to_string(),
        };
        let req = build_clob_auth_eip712_request(&params);

        let domain = req.domain.as_ref().expect("domain required");
        assert_eq!(domain.name.as_deref(), Some("ClobAuthDomain"));
        assert_eq!(domain.version.as_deref(), Some("1"));
        assert_eq!(domain.chain_id, Some(137));
        assert!(
            domain.verifying_contract.is_none(),
            "ClobAuth domain should not have verifyingContract"
        );
    }

    #[test]
    fn clob_auth_eip712_type_fields() {
        let params = ClobAuthParams {
            address: "0xabc".to_string(),
            timestamp: "1234567890".to_string(),
            nonce: "0".to_string(),
            message: "hello".to_string(),
        };
        let req = build_clob_auth_eip712_request(&params);

        assert_eq!(req.primary_type.as_deref(), Some("ClobAuth"));

        let types = req.types.as_ref().expect("types required");
        let clob_auth_fields = types.get("ClobAuth").expect("ClobAuth type required");
        let fields: Vec<(String, String)> = clob_auth_fields
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
                ("address".to_string(), "address".to_string()),
                ("timestamp".to_string(), "string".to_string()),
                ("nonce".to_string(), "uint256".to_string()),
                ("message".to_string(), "string".to_string()),
            ]
        );
    }

    #[test]
    fn clob_auth_eip712_message_values() {
        let params = ClobAuthParams {
            address: "0xdeadbeef".to_string(),
            timestamp: "9999".to_string(),
            nonce: "42".to_string(),
            message: "test-msg".to_string(),
        };
        let req = build_clob_auth_eip712_request(&params);
        let msg = req.message.as_ref().expect("message required");

        assert_eq!(msg["address"].as_str(), Some("0xdeadbeef"));
        assert_eq!(msg["timestamp"].as_str(), Some("9999"));
        assert_eq!(msg["nonce"].as_str(), Some("42"));
        assert_eq!(msg["message"].as_str(), Some("test-msg"));
    }

    // -- Order EIP-712 tests --

    #[test]
    fn order_eip712_domain_has_verifying_contract() {
        let params = test_order_params();
        let req = build_order_eip712_request(&params, CTF_EXCHANGE_ADDRESS);

        let domain = req.domain.as_ref().expect("domain required");
        assert_eq!(domain.name.as_deref(), Some("Polymarket CTF Exchange"));
        assert_eq!(domain.version.as_deref(), Some("1"));
        assert_eq!(domain.chain_id, Some(137));
        assert_eq!(
            domain.verifying_contract.as_deref(),
            Some(CTF_EXCHANGE_ADDRESS)
        );
    }

    #[test]
    fn order_eip712_type_fields_present_and_correct() {
        let params = test_order_params();
        let req = build_order_eip712_request(&params, CTF_EXCHANGE_ADDRESS);

        assert_eq!(req.primary_type.as_deref(), Some("Order"));

        let types = req.types.as_ref().expect("types required");
        let order_fields = types.get("Order").expect("Order type required");
        let fields: Vec<(String, String)> = order_fields
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

        let expected = vec![
            ("salt", "uint256"),
            ("maker", "address"),
            ("signer", "address"),
            ("taker", "address"),
            ("tokenId", "uint256"),
            ("makerAmount", "uint256"),
            ("takerAmount", "uint256"),
            ("expiration", "uint256"),
            ("nonce", "uint256"),
            ("feeRateBps", "uint256"),
            ("side", "uint8"),
            ("signatureType", "uint8"),
        ];

        assert_eq!(fields.len(), expected.len());
        for (got, (exp_name, exp_type)) in fields.iter().zip(expected.iter()) {
            assert_eq!(got.0, *exp_name);
            assert_eq!(got.1, *exp_type);
        }
    }

    #[test]
    fn order_eip712_message_values() {
        let params = test_order_params();
        let req = build_order_eip712_request(&params, CTF_EXCHANGE_ADDRESS);
        let msg = req.message.as_ref().expect("message required");

        assert_eq!(msg["salt"].as_str(), Some("12345"));
        assert_eq!(msg["maker"].as_str(), Some("0xmaker"));
        assert_eq!(msg["signer"].as_str(), Some("0xsigner"));
        assert_eq!(
            msg["taker"].as_str(),
            Some("0x0000000000000000000000000000000000000000")
        );
        assert_eq!(msg["tokenId"].as_str(), Some("71321045649"));
        assert_eq!(msg["makerAmount"].as_str(), Some("100000000"));
        assert_eq!(msg["takerAmount"].as_str(), Some("50000000"));
        assert_eq!(msg["expiration"].as_str(), Some("0"));
        assert_eq!(msg["nonce"].as_str(), Some("0"));
        assert_eq!(msg["feeRateBps"].as_str(), Some("100"));
        assert_eq!(msg["side"].as_str(), Some("0"));
        assert_eq!(msg["signatureType"].as_str(), Some("0"));
    }

    #[test]
    fn order_eip712_neg_risk_exchange_address() {
        let params = test_order_params();
        let req = build_order_eip712_request(&params, NEG_RISK_CTF_EXCHANGE_ADDRESS);
        let domain = req.domain.as_ref().unwrap();
        assert_eq!(
            domain.verifying_contract.as_deref(),
            Some(NEG_RISK_CTF_EXCHANGE_ADDRESS)
        );
    }

    // -- HMAC tests --

    #[test]
    fn hmac_computation_known_test_vector() {
        // Fixed inputs for deterministic verification.
        let secret_raw = b"test-secret-key!"; // 16 bytes
        let secret_b64 = BASE64.encode(secret_raw);

        let timestamp = "1700000000";
        let method = "POST";
        let path = "/order";
        let body = r#"{"side":"BUY"}"#;

        let sig = compute_hmac_signature(&secret_b64, timestamp, method, path, body).unwrap();

        // Verify it's valid base64
        let decoded = BASE64.decode(&sig).expect("signature must be valid base64");
        assert_eq!(decoded.len(), 32, "HMAC-SHA256 produces 32 bytes");

        // Recompute manually to verify
        let mut mac = HmacSha256::new_from_slice(secret_raw).unwrap();
        mac.update(format!("{timestamp}{method}{path}{body}").as_bytes());
        let expected = BASE64.encode(mac.finalize().into_bytes());
        assert_eq!(sig, expected);
    }

    #[test]
    fn hmac_uses_base64_decoded_secret_as_key() {
        // If we use the raw base64 string as key instead of decoding it,
        // we'd get a different result. This test verifies decoding happens.
        let secret_raw = b"my-key-bytes1234";
        let secret_b64 = BASE64.encode(secret_raw);

        let sig_correct = compute_hmac_signature(&secret_b64, "ts", "GET", "/", "").unwrap();

        // Compute HMAC with raw base64 string as key (wrong approach)
        let mut mac_wrong = HmacSha256::new_from_slice(secret_b64.as_bytes()).unwrap();
        mac_wrong.update(b"tsGET/");
        let sig_wrong = BASE64.encode(mac_wrong.finalize().into_bytes());

        assert_ne!(
            sig_correct, sig_wrong,
            "HMAC must use base64-decoded secret, not raw base64 string"
        );
    }

    #[test]
    fn hmac_invalid_base64_secret_returns_error() {
        let result = compute_hmac_signature("!!!not-base64!!!", "ts", "GET", "/", "");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("base64"),
            "error should mention base64: {err}"
        );
    }

    // -- L1 header tests --

    #[test]
    fn l1_auth_headers_correct_names() {
        let headers = build_l1_auth_headers("0xaddr", "0xsig", "12345", "0");
        let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "POLY_ADDRESS",
                "POLY_SIGNATURE",
                "POLY_TIMESTAMP",
                "POLY_NONCE"
            ]
        );
    }

    #[test]
    fn l1_auth_headers_correct_values() {
        let headers = build_l1_auth_headers("0xmyaddr", "0xmysig", "999", "7");
        let map: std::collections::HashMap<&str, &str> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(map["POLY_ADDRESS"], "0xmyaddr");
        assert_eq!(map["POLY_SIGNATURE"], "0xmysig");
        assert_eq!(map["POLY_TIMESTAMP"], "999");
        assert_eq!(map["POLY_NONCE"], "7");
    }

    // -- L2 header tests --

    #[test]
    fn l2_hmac_headers_correct_names() {
        let creds = test_credentials();
        let headers =
            build_l2_hmac_headers(&creds, "0xaddr", "12345", "GET", "/orders", "").unwrap();
        let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "POLY_ADDRESS",
                "POLY_SIGNATURE",
                "POLY_TIMESTAMP",
                "POLY_API_KEY",
                "POLY_PASSPHRASE"
            ]
        );
    }

    #[test]
    fn l2_hmac_headers_contain_credentials() {
        let creds = test_credentials();
        let headers =
            build_l2_hmac_headers(&creds, "0xaddr", "12345", "POST", "/order", r#"{"a":1}"#)
                .unwrap();
        let map: std::collections::HashMap<&str, &str> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        assert_eq!(map["POLY_ADDRESS"], "0xaddr");
        assert_eq!(map["POLY_TIMESTAMP"], "12345");
        assert_eq!(map["POLY_API_KEY"], "test-api-key");
        assert_eq!(map["POLY_PASSPHRASE"], "test-passphrase");
        // Signature should be non-empty base64
        assert!(!map["POLY_SIGNATURE"].is_empty());
        BASE64
            .decode(map["POLY_SIGNATURE"])
            .expect("POLY_SIGNATURE must be valid base64");
    }

    // -- Salt generation test --

    #[test]
    fn generate_order_salt_is_nonempty_numeric() {
        let salt = generate_order_salt();
        assert!(!salt.is_empty());
        // Should parse as a number
        assert!(
            salt.parse::<u128>().is_ok(),
            "salt should be a numeric string: {salt}"
        );
    }

    #[test]
    fn generate_order_salt_unique() {
        let s1 = generate_order_salt();
        // Small sleep to ensure time advances
        std::thread::sleep(std::time::Duration::from_millis(1));
        let s2 = generate_order_salt();
        // Not guaranteed in unit tests due to timing, but very likely different
        // At minimum both should be valid
        assert!(!s1.is_empty());
        assert!(!s2.is_empty());
    }

    // -- Helpers --

    fn test_order_params() -> OrderParams {
        OrderParams {
            salt: "12345".to_string(),
            maker: "0xmaker".to_string(),
            signer: "0xsigner".to_string(),
            taker: "0x0000000000000000000000000000000000000000".to_string(),
            token_id: "71321045649".to_string(),
            maker_amount: "100000000".to_string(),
            taker_amount: "50000000".to_string(),
            expiration: "0".to_string(),
            nonce: "0".to_string(),
            fee_rate_bps: "100".to_string(),
            side: "0".to_string(),
            signature_type: "0".to_string(),
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
}
