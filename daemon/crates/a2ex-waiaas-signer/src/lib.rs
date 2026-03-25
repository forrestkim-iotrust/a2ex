use std::time::Duration;

use a2ex_signer_bridge::{SignedPayload, SignerBridge, SignerBridgeError, TypedDataSignRequest};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// WAIaaS sign-message request body.
///
/// WAIaaS 2.10.0+: `message` must be a string. For typedData signing,
/// the EIP-712 payload goes in the separate `typed_data` field.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WaiaasSignMessageRequest {
    wallet_id: String,
    network: String,
    message: String,
    sign_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    typed_data: Option<serde_json::Value>,
}

/// WAIaaS sign-message response body.
#[derive(Debug, Deserialize)]
struct WaiaasSignMessageResponse {
    signature: String,
}

/// A [`SignerBridge`] implementation that delegates EIP-712 typed-data signing
/// to a WAIaaS `POST /v1/transactions/sign-message` endpoint.
///
/// The bridge sends structured EIP-712 fields (domain, types, primaryType,
/// message) inside the `message` object and sets `signType: "typedData"`.
///
/// ## Error Mapping
/// - 401 / 403 → [`SignerBridgeError::AuthError`]
/// - Other 4xx / 5xx → [`SignerBridgeError::HttpError`]
/// - Timeout / connection failure → [`SignerBridgeError::NetworkError`]
#[derive(Debug, Clone)]
pub struct WaiaasSignerBridge {
    base_url: String,
    hot_session_token: String,
    wallet_id: String,
    network: String,
    client: Client,
}

impl WaiaasSignerBridge {
    /// Create a new bridge.
    ///
    /// Builds a [`reqwest::Client`] with a 5-second timeout.
    pub fn new(
        base_url: impl Into<String>,
        hot_session_token: impl Into<String>,
        wallet_id: impl Into<String>,
        network: impl Into<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client builds");
        Self {
            base_url: base_url.into(),
            hot_session_token: hot_session_token.into(),
            wallet_id: wallet_id.into(),
            network: network.into(),
            client,
        }
    }

    /// Build the EIP-712 message object from a [`TypedDataSignRequest`].
    fn build_message_value(req: &TypedDataSignRequest) -> serde_json::Value {
        let mut msg = serde_json::Map::new();
        if let Some(domain) = &req.domain {
            msg.insert(
                "domain".to_owned(),
                serde_json::to_value(domain).unwrap_or_default(),
            );
        }
        if let Some(types) = &req.types {
            msg.insert("types".to_owned(), types.clone());
        }
        if let Some(primary_type) = &req.primary_type {
            msg.insert(
                "primaryType".to_owned(),
                serde_json::Value::String(primary_type.clone()),
            );
        }
        if let Some(message) = &req.message {
            msg.insert("message".to_owned(), message.clone());
        }
        serde_json::Value::Object(msg)
    }
}

#[async_trait]
impl SignerBridge for WaiaasSignerBridge {
    async fn sign_typed_data(
        &self,
        req: TypedDataSignRequest,
    ) -> Result<SignedPayload, SignerBridgeError> {
        let url = format!(
            "{}/v1/transactions/sign-message",
            self.base_url.trim_end_matches('/')
        );

        tracing::info!(
            wallet_id_prefix = &self.wallet_id[..self.wallet_id.len().min(8)],
            network = %self.network,
            "waiaas_sign_typed_data: sending request"
        );

        let body = WaiaasSignMessageRequest {
            wallet_id: self.wallet_id.clone(),
            network: self.network.clone(),
            message: "typedData".to_owned(),
            sign_type: "typedData".to_owned(),
            typed_data: Some(Self::build_message_value(&req)),
        };

        tracing::debug!(body = %serde_json::to_string(&body).unwrap_or_default(), "waiaas_sign_typed_data: request body");

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header(
                "Authorization",
                format!("Bearer {}", self.hot_session_token),
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    tracing::error!(error = %e, "waiaas_sign_typed_data: request timeout");
                    SignerBridgeError::NetworkError {
                        message: format!("request timeout: {e}"),
                    }
                } else if e.is_connect() {
                    tracing::error!(error = %e, "waiaas_sign_typed_data: connection failed");
                    SignerBridgeError::NetworkError {
                        message: format!("connection failed: {e}"),
                    }
                } else {
                    tracing::error!(error = %e, "waiaas_sign_typed_data: network error");
                    SignerBridgeError::NetworkError {
                        message: format!("request failed: {e}"),
                    }
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response.text().await.unwrap_or_default();

            if status_code == 401 || status_code == 403 {
                tracing::warn!(status = status_code, "waiaas_sign_typed_data: auth error");
                return Err(SignerBridgeError::AuthError {
                    message: format!("HTTP {status_code}: {body_text}"),
                });
            }

            tracing::warn!(status = status_code, "waiaas_sign_typed_data: HTTP error");
            return Err(SignerBridgeError::HttpError {
                status: status_code,
                message: body_text,
            });
        }

        let resp: WaiaasSignMessageResponse =
            response
                .json()
                .await
                .map_err(|e| SignerBridgeError::HttpError {
                    status: 200,
                    message: format!("failed to parse response: {e}"),
                })?;

        Ok(SignedPayload::with_hex(resp.signature))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2ex_signer_bridge::Eip712Domain;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_eip712_request() -> TypedDataSignRequest {
        TypedDataSignRequest {
            payload: vec![],
            domain: Some(Eip712Domain {
                name: Some("Polymarket".into()),
                version: Some("1".into()),
                chain_id: Some(137),
                verifying_contract: Some("0xC5d563A36AE78145C45a50134d48A1215220f80a".into()),
            }),
            types: Some(json!({
                "Order": [
                    {"name": "maker", "type": "address"},
                    {"name": "salt", "type": "uint256"}
                ]
            })),
            primary_type: Some("Order".into()),
            message: Some(json!({
                "maker": "0xdef456",
                "salt": "12345"
            })),
        }
    }

    fn bridge_for(server_uri: &str) -> WaiaasSignerBridge {
        WaiaasSignerBridge::new(
            server_uri,
            "test-session-token",
            "wallet-abc-123",
            "polygon",
        )
    }

    #[tokio::test]
    async fn sign_typed_data_success() {
        let server = MockServer::start().await;
        let sig = "0xaabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd1122334400";

        Mock::given(method("POST"))
            .and(path("/v1/transactions/sign-message"))
            .and(header("Authorization", "Bearer test-session-token"))
            .and(header("Content-Type", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "signature": sig,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let bridge = bridge_for(&server.uri());
        let result = bridge.sign_typed_data(test_eip712_request()).await;
        let signed = result.expect("should succeed");

        assert_eq!(signed.signature_hex.as_deref(), Some(sig));
        assert!(!signed.bytes.is_empty());
    }

    #[tokio::test]
    async fn sign_typed_data_auth_error_401() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/transactions/sign-message"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"error": "unauthorized"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let bridge = bridge_for(&server.uri());
        let result = bridge.sign_typed_data(test_eip712_request()).await;
        let err = result.unwrap_err();

        match err {
            SignerBridgeError::AuthError { message } => {
                assert!(
                    message.contains("401"),
                    "message should contain status: {message}"
                );
            }
            other => panic!("expected AuthError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sign_typed_data_auth_error_403() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/transactions/sign-message"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error": "forbidden"})))
            .expect(1)
            .mount(&server)
            .await;

        let bridge = bridge_for(&server.uri());
        let result = bridge.sign_typed_data(test_eip712_request()).await;

        match result.unwrap_err() {
            SignerBridgeError::AuthError { message } => {
                assert!(message.contains("403"));
            }
            other => panic!("expected AuthError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sign_typed_data_server_error_500() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/transactions/sign-message"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
            .expect(1)
            .mount(&server)
            .await;

        let bridge = bridge_for(&server.uri());
        let result = bridge.sign_typed_data(test_eip712_request()).await;

        match result.unwrap_err() {
            SignerBridgeError::HttpError { status, message } => {
                assert_eq!(status, 500);
                assert!(message.contains("internal server error"));
            }
            other => panic!("expected HttpError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sign_typed_data_malformed_response() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/transactions/sign-message"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"not_signature": "oops"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let bridge = bridge_for(&server.uri());
        let result = bridge.sign_typed_data(test_eip712_request()).await;

        match result.unwrap_err() {
            SignerBridgeError::HttpError { status, message } => {
                assert_eq!(status, 200);
                assert!(
                    message.contains("parse"),
                    "should mention parse failure: {message}"
                );
            }
            other => panic!("expected HttpError for malformed response, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sign_typed_data_network_error() {
        // Point to a port that is not listening.
        let bridge = WaiaasSignerBridge::new("http://127.0.0.1:1", "token", "wallet", "polygon");

        let result = bridge.sign_typed_data(test_eip712_request()).await;

        match result.unwrap_err() {
            SignerBridgeError::NetworkError { message } => {
                assert!(!message.is_empty());
            }
            other => panic!("expected NetworkError, got: {other:?}"),
        }
    }

    /// Verify the exact JSON body shape sent to WAIaaS:
    /// WAIaaS 2.10.0+: `message` is a string, EIP-712 payload goes in `typedData`.
    /// - `walletId`, `network`, `signType: "typedData"`, `message: string`
    /// - `typedData` object with `domain`, `types`, `primaryType`, `message`
    #[tokio::test]
    async fn sign_typed_data_request_body_shape() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/transactions/sign-message"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "signature": "0xdeadbeef"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let bridge = bridge_for(&server.uri());
        let _ = bridge.sign_typed_data(test_eip712_request()).await;

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);

        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("body should be valid JSON");

        // Top-level fields
        assert_eq!(body["walletId"], "wallet-abc-123");
        assert_eq!(body["network"], "polygon");
        assert_eq!(body["signType"], "typedData");
        assert!(body["message"].is_string(), "message must be a string for WAIaaS 2.10.0+");

        // typedData contains EIP-712 fields
        let td = &body["typedData"];
        assert!(td.is_object(), "typedData should be an object");

        // domain
        let domain = &td["domain"];
        assert_eq!(domain["name"], "Polymarket");
        assert_eq!(domain["version"], "1");
        assert_eq!(domain["chainId"], 137);
        assert_eq!(
            domain["verifyingContract"],
            "0xC5d563A36AE78145C45a50134d48A1215220f80a"
        );

        // types
        assert!(td["types"].is_object());
        assert!(td["types"]["Order"].is_array());

        // primaryType
        assert_eq!(td["primaryType"], "Order");

        // message (inner)
        assert_eq!(td["message"]["maker"], "0xdef456");
        assert_eq!(td["message"]["salt"], "12345");
    }
}
