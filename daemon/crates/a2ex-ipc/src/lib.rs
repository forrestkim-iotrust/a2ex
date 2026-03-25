use std::path::{Path, PathBuf};

use a2ex_control::{
    IntentAcknowledgement, RegisterStrategyRequest, StrategyAcknowledgement, SubmitIntentRequest,
};
use futures_util::{SinkExt, StreamExt};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, Name, ToFsName};
#[cfg(windows)]
use interprocess::{
    local_socket::{Name, ToFsName},
    os::windows::local_socket::NamedPipe,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub const JSON_RPC_VERSION: &str = "2.0";
pub const DAEMON_CONTROL_METHOD: &str = "daemon.authorizeExecution";
pub const PLATFORM_SUBMIT_INTENT_METHOD: &str = "platform.submitIntent";
pub const PLATFORM_REGISTER_STRATEGY_METHOD: &str = "platform.registerStrategy";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlatformMethod {
    #[serde(rename = "platform.submitIntent")]
    SubmitIntent,
    #[serde(rename = "platform.registerStrategy")]
    RegisterStrategy,
}

impl PlatformMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubmitIntent => PLATFORM_SUBMIT_INTENT_METHOD,
            Self::RegisterStrategy => PLATFORM_REGISTER_STRATEGY_METHOD,
        }
    }
}

pub type SubmitIntentRpcRequest = JsonRpcRequest<SubmitIntentRequest>;
pub type RegisterStrategyRpcRequest = JsonRpcRequest<RegisterStrategyRequest>;
pub type SubmitIntentRpcResponse = JsonRpcResponse<IntentAcknowledgement>;
pub type RegisterStrategyRpcResponse = JsonRpcResponse<StrategyAcknowledgement>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalTransport {
    UnixDomainSocket,
    NamedPipe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalControlEndpoint {
    path: PathBuf,
}

impl LocalControlEndpoint {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn transport(&self) -> LocalTransport {
        #[cfg(unix)]
        {
            LocalTransport::UnixDomainSocket
        }

        #[cfg(windows)]
        {
            LocalTransport::NamedPipe
        }
    }

    pub fn socket_name(&self) -> std::io::Result<Name<'_>> {
        #[cfg(unix)]
        {
            self.path.as_path().to_fs_name::<GenericFilePath>()
        }

        #[cfg(windows)]
        {
            self.path.as_path().to_fs_name::<NamedPipe>()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcRequest<P> {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: P,
}

impl<P> JsonRpcRequest<P> {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: P) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcSuccess<R> {
    pub jsonrpc: String,
    pub id: String,
    pub result: R,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcFailure {
    pub jsonrpc: String,
    pub id: String,
    pub error: JsonRpcError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse<R> {
    Success(JsonRpcSuccess<R>),
    Failure(JsonRpcFailure),
}

impl<R> JsonRpcResponse<R> {
    pub fn success(id: impl Into<String>, result: R) -> Self {
        Self::Success(JsonRpcSuccess {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: id.into(),
            result,
        })
    }

    pub fn failure(id: impl Into<String>, code: i32, message: impl Into<String>) -> Self {
        Self::Failure(JsonRpcFailure {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: id.into(),
            error: JsonRpcError {
                code,
                message: message.into(),
            },
        })
    }
}

pub fn frame_transport<T>(io: T) -> Framed<T, LengthDelimitedCodec>
where
    T: AsyncRead + AsyncWrite,
{
    Framed::new(io, LengthDelimitedCodec::new())
}

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("failed to serialize JSON-RPC payload")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize JSON-RPC payload")]
    Deserialize(#[source] serde_json::Error),
    #[error("failed to write framed IPC payload")]
    Send(#[source] std::io::Error),
    #[error("failed to read framed IPC payload")]
    Receive(#[source] std::io::Error),
    #[error("framed IPC stream closed before a full JSON-RPC message arrived")]
    Closed,
}

pub async fn send_json_message<T, M>(
    framed: &mut Framed<T, LengthDelimitedCodec>,
    message: &M,
) -> Result<(), IpcError>
where
    T: AsyncRead + AsyncWrite + Unpin,
    M: Serialize,
{
    let payload = serde_json::to_vec(message).map_err(IpcError::Serialize)?;
    framed.send(payload.into()).await.map_err(IpcError::Send)
}

pub async fn recv_json_message<T, M>(
    framed: &mut Framed<T, LengthDelimitedCodec>,
) -> Result<M, IpcError>
where
    T: AsyncRead + AsyncWrite + Unpin,
    M: DeserializeOwned,
{
    let payload = framed
        .next()
        .await
        .ok_or(IpcError::Closed)?
        .map_err(IpcError::Receive)?;

    serde_json::from_slice(payload.as_ref()).map_err(IpcError::Deserialize)
}

pub fn parse_submit_intent_request(
    request: JsonRpcRequest<serde_json::Value>,
) -> Result<SubmitIntentRpcRequest, IpcError> {
    serde_json::from_value(serde_json::to_value(request).map_err(IpcError::Serialize)?)
        .map_err(IpcError::Deserialize)
}

pub fn parse_register_strategy_request(
    request: JsonRpcRequest<serde_json::Value>,
) -> Result<RegisterStrategyRpcRequest, IpcError> {
    serde_json::from_value(serde_json::to_value(request).map_err(IpcError::Serialize)?)
        .map_err(IpcError::Deserialize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Ping {
        message: String,
    }

    #[tokio::test]
    async fn reports_local_socket_transport_instead_of_http() {
        let endpoint = LocalControlEndpoint::new("/tmp/a2ex-control.sock");

        assert!(matches!(
            endpoint.transport(),
            LocalTransport::UnixDomainSocket | LocalTransport::NamedPipe
        ));
    }

    #[tokio::test]
    async fn round_trips_length_delimited_json_rpc_messages() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let mut client = frame_transport(client_io);
        let mut server = frame_transport(server_io);
        let request = JsonRpcRequest::new(
            "req-1",
            DAEMON_CONTROL_METHOD,
            Ping {
                message: "local-only".to_owned(),
            },
        );

        send_json_message(&mut client, &request)
            .await
            .expect("client request writes to the local framed transport");

        let received: JsonRpcRequest<Ping> = recv_json_message(&mut server)
            .await
            .expect("server reads the same framed request");

        assert_eq!(received, request);
    }

    #[test]
    fn parses_submit_intent_bindings_from_generic_json_rpc_request() {
        let request = JsonRpcRequest::new(
            "req-intent",
            PLATFORM_SUBMIT_INTENT_METHOD,
            serde_json::json!({
                "request_id": "req-intent-1",
                "request_kind": "intent",
                "source_agent_id": "agent.test",
                "submitted_at": "2026-03-12T00:00:00Z",
                "payload": {
                    "intent_id": "intent-1",
                    "intent_type": "open_exposure",
                    "objective": {
                        "domain": "prediction_market",
                        "target_market": "fed-cut-2026",
                        "side": "yes",
                        "target_notional_usd": 25
                    },
                    "constraints": {
                        "allowed_venues": ["polymarket"],
                        "max_slippage_bps": 80,
                        "max_fee_usd": 5,
                        "urgency": "normal"
                    },
                    "funding": {
                        "preferred_asset": "USDC",
                        "source_chain": "base"
                    },
                    "post_actions": []
                },
                "rationale": { "summary": "typed binding" },
                "execution_preferences": {
                    "preview_only": false,
                    "allow_fast_path": true
                }
            }),
        );

        let parsed = parse_submit_intent_request(request).expect("request parses");
        assert_eq!(parsed.method, PLATFORM_SUBMIT_INTENT_METHOD);
        assert_eq!(parsed.params.payload.intent_id, "intent-1");
    }
}
