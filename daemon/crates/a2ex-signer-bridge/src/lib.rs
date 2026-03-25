use std::sync::Arc;

use a2ex_ipc::LocalTransport;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxSignRequest {
    pub payload: Vec<u8>,
}

/// EIP-712 domain separator fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Eip712Domain {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifying_contract: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TypedDataSignRequest {
    pub payload: Vec<u8>,
    /// Structured EIP-712 domain (optional — set for typed-data signing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<Eip712Domain>,
    /// EIP-712 type definitions as a JSON object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<serde_json::Value>,
    /// EIP-712 primary type name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_type: Option<String>,
    /// EIP-712 message payload as a JSON object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytesSignRequest {
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTx {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPayload {
    pub bytes: Vec<u8>,
    /// Hex-encoded signature (e.g. `0x...`), when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_hex: Option<String>,
}

impl SignedPayload {
    /// Create a `SignedPayload` from a hex-encoded signature string.
    /// The hex string is also decoded into `bytes` (stripping a leading `0x` if present).
    pub fn with_hex(signature_hex: String) -> Self {
        let stripped = signature_hex.strip_prefix("0x").unwrap_or(&signature_hex);
        let bytes = hex_decode(stripped);
        Self {
            bytes,
            signature_hex: Some(signature_hex),
        }
    }
}

/// Best-effort hex decoding — returns empty vec on invalid input.
fn hex_decode(hex: &str) -> Vec<u8> {
    if hex.len() % 2 != 0 {
        return Vec::new();
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub action_id: String,
    pub action_kind: String,
    pub reservation_id: String,
    pub notional_usd: u64,
    pub origin_transport: LocalTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignerBridgeRequestRecord {
    pub request_kind: String,
    pub action_id: String,
    pub reservation_id: String,
    pub action_kind: String,
    pub notional_usd: u64,
    pub origin_transport: LocalTransport,
}

impl SignerBridgeRequestRecord {
    pub fn approval(req: ApprovalRequest) -> Self {
        Self {
            request_kind: "approval".to_owned(),
            action_id: req.action_id,
            reservation_id: req.reservation_id,
            action_kind: req.action_kind,
            notional_usd: req.notional_usd,
            origin_transport: req.origin_transport,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalResult {
    pub approved: bool,
    pub audit: SignerBridgeRequestRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPeerIdentity {
    pub transport: LocalTransport,
    pub same_user: bool,
    pub runtime_owned_endpoint: bool,
}

impl LocalPeerIdentity {
    pub fn for_tests(same_user: bool, runtime_owned_endpoint: bool) -> Self {
        Self {
            transport: if cfg!(unix) {
                LocalTransport::UnixDomainSocket
            } else {
                LocalTransport::NamedPipe
            },
            same_user,
            runtime_owned_endpoint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPeerValidator {
    allowed_transports: Vec<LocalTransport>,
    require_same_user: bool,
    require_runtime_owned_endpoint: bool,
}

impl LocalPeerValidator {
    pub fn strict_local_only() -> Self {
        Self {
            allowed_transports: vec![LocalTransport::UnixDomainSocket, LocalTransport::NamedPipe],
            require_same_user: true,
            require_runtime_owned_endpoint: true,
        }
    }

    pub fn validate(&self, peer: &LocalPeerIdentity) -> Result<(), SignerBridgeError> {
        if !self.allowed_transports.contains(&peer.transport) {
            return Err(SignerBridgeError::PeerValidation {
                reason: format!("unsupported local signer transport: {:?}", peer.transport),
            });
        }

        if self.require_same_user && !peer.same_user {
            return Err(SignerBridgeError::PeerValidation {
                reason: "local signer bridge peer validation failed: peer is not owned by the runtime user".to_owned(),
            });
        }

        if self.require_runtime_owned_endpoint && !peer.runtime_owned_endpoint {
            return Err(SignerBridgeError::PeerValidation {
                reason:
                    "local signer bridge peer validation failed: endpoint ownership check failed"
                        .to_owned(),
            });
        }

        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SignerBridgeError {
    #[error("signer request requires an explicit local bridge contract")]
    ExplicitBridgeRequired,
    #[error("{reason}")]
    PeerValidation { reason: String },
    #[error("{operation} is not implemented by this signer bridge")]
    UnsupportedOperation { operation: &'static str },
    #[error("HTTP error {status}: {message}")]
    HttpError { status: u16, message: String },
    #[error("authentication error: {message}")]
    AuthError { message: String },
    #[error("network error: {message}")]
    NetworkError { message: String },
}

#[async_trait]
pub trait SignerBridge: Send + Sync {
    async fn sign_transaction(&self, _req: TxSignRequest) -> Result<SignedTx, SignerBridgeError> {
        Err(SignerBridgeError::UnsupportedOperation {
            operation: "sign_transaction",
        })
    }

    async fn sign_typed_data(
        &self,
        _req: TypedDataSignRequest,
    ) -> Result<SignedPayload, SignerBridgeError> {
        Err(SignerBridgeError::UnsupportedOperation {
            operation: "sign_typed_data",
        })
    }

    async fn sign_bytes(&self, _req: BytesSignRequest) -> Result<SignedPayload, SignerBridgeError> {
        Err(SignerBridgeError::UnsupportedOperation {
            operation: "sign_bytes",
        })
    }

    async fn request_approval(
        &self,
        _req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        Err(SignerBridgeError::UnsupportedOperation {
            operation: "request_approval",
        })
    }
}

#[async_trait]
pub trait ValidatedSignerBridge: Send + Sync {
    async fn request_approval_from_peer(
        &self,
        peer: &LocalPeerIdentity,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError>;

    async fn sign_transaction_from_peer(
        &self,
        peer: &LocalPeerIdentity,
        req: TxSignRequest,
    ) -> Result<SignedTx, SignerBridgeError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSignerBridge;

#[async_trait]
impl ValidatedSignerBridge for NoopSignerBridge {
    async fn request_approval_from_peer(
        &self,
        _peer: &LocalPeerIdentity,
        _req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        Err(SignerBridgeError::ExplicitBridgeRequired)
    }

    async fn sign_transaction_from_peer(
        &self,
        _peer: &LocalPeerIdentity,
        _req: TxSignRequest,
    ) -> Result<SignedTx, SignerBridgeError> {
        Err(SignerBridgeError::ExplicitBridgeRequired)
    }
}

#[derive(Debug, Clone)]
pub struct LocalSignerBridgeClient<B> {
    bridge: Arc<B>,
    validator: LocalPeerValidator,
}

impl<B> LocalSignerBridgeClient<B> {
    pub fn new(bridge: Arc<B>, validator: LocalPeerValidator) -> Self {
        Self { bridge, validator }
    }
}

#[async_trait]
impl<B> ValidatedSignerBridge for LocalSignerBridgeClient<B>
where
    B: SignerBridge,
{
    async fn request_approval_from_peer(
        &self,
        peer: &LocalPeerIdentity,
        req: ApprovalRequest,
    ) -> Result<ApprovalResult, SignerBridgeError> {
        self.validator.validate(peer)?;
        self.bridge.request_approval(req).await
    }

    async fn sign_transaction_from_peer(
        &self,
        peer: &LocalPeerIdentity,
        req: TxSignRequest,
    ) -> Result<SignedTx, SignerBridgeError> {
        self.validator.validate(peer)?;
        self.bridge.sign_transaction(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_data_sign_request_payload_only_roundtrip() {
        let req = TypedDataSignRequest {
            payload: vec![1, 2, 3],
            ..Default::default()
        };
        let json = serde_json::to_string(&req).unwrap();
        let deser: TypedDataSignRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.payload, vec![1, 2, 3]);
        assert!(deser.domain.is_none());
        assert!(deser.types.is_none());
        assert!(deser.primary_type.is_none());
        assert!(deser.message.is_none());
    }

    #[test]
    fn typed_data_sign_request_full_eip712_roundtrip() {
        let domain = Eip712Domain {
            name: Some("Polymarket".into()),
            version: Some("1".into()),
            chain_id: Some(137),
            verifying_contract: Some("0xabc".into()),
        };
        let types = json!({
            "Order": [
                {"name": "maker", "type": "address"},
                {"name": "salt", "type": "uint256"}
            ]
        });
        let message = json!({
            "maker": "0xdef",
            "salt": "12345"
        });
        let req = TypedDataSignRequest {
            payload: vec![],
            domain: Some(domain.clone()),
            types: Some(types.clone()),
            primary_type: Some("Order".into()),
            message: Some(message.clone()),
        };
        let json_str = serde_json::to_string(&req).unwrap();
        let deser: TypedDataSignRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            deser.domain.as_ref().unwrap().name.as_deref(),
            Some("Polymarket")
        );
        assert_eq!(deser.domain.as_ref().unwrap().chain_id, Some(137));
        assert_eq!(deser.types, Some(types));
        assert_eq!(deser.primary_type.as_deref(), Some("Order"));
        assert_eq!(deser.message, Some(message));
    }

    #[test]
    fn signed_payload_with_hex_stores_both() {
        let hex = "0xabcdef01".to_string();
        let sp = SignedPayload::with_hex(hex.clone());
        assert_eq!(sp.signature_hex.as_deref(), Some("0xabcdef01"));
        assert_eq!(sp.bytes, vec![0xab, 0xcd, 0xef, 0x01]);
    }

    #[test]
    fn signed_payload_with_hex_no_prefix() {
        let sp = SignedPayload::with_hex("deadbeef".to_string());
        assert_eq!(sp.signature_hex.as_deref(), Some("deadbeef"));
        assert_eq!(sp.bytes, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn error_variants_display() {
        let http = SignerBridgeError::HttpError {
            status: 403,
            message: "forbidden".into(),
        };
        assert!(http.to_string().contains("403"));

        let auth = SignerBridgeError::AuthError {
            message: "bad token".into(),
        };
        assert!(auth.to_string().contains("bad token"));

        let net = SignerBridgeError::NetworkError {
            message: "timeout".into(),
        };
        assert!(net.to_string().contains("timeout"));
    }

    #[test]
    fn backward_compat_payload_only_construction() {
        // Existing code pattern should still work with Default
        let req = TypedDataSignRequest {
            payload: vec![0xff],
            ..Default::default()
        };
        assert_eq!(req.payload, vec![0xff]);
    }
}
