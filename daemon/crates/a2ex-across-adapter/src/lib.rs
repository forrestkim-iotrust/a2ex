use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod transport;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AcrossAdapterError {
    #[error("across transport error: {message}")]
    Transport { message: String },
}

impl AcrossAdapterError {
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossApproval {
    pub token: String,
    pub spender: String,
    pub allowance_target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossBridgeQuoteRequest {
    pub asset: String,
    pub amount_usd: u64,
    pub source_chain: String,
    pub destination_chain: String,
    /// Depositor address (required by Across Swap API).
    #[serde(default)]
    pub depositor: Option<String>,
    /// Recipient address (defaults to depositor if not set).
    #[serde(default)]
    pub recipient: Option<String>,
    /// Output token address (defaults to same as input for bridgeable-to-bridgeable).
    #[serde(default)]
    pub output_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapTx {
    pub to: String,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalTx {
    pub to: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossBridgeQuote {
    pub route_id: String,
    pub bridge_fee_usd: u64,
    pub expected_fill_seconds: u64,
    pub approval: AcrossApproval,
    /// Raw approval transactions from Across API (hex-encoded calldata).
    #[serde(default)]
    pub approval_txns: Vec<ApprovalTx>,
    #[serde(default)]
    pub calldata: Option<String>,
    #[serde(default)]
    pub swap_tx: Option<SwapTx>,
    #[serde(default)]
    pub input_amount: Option<String>,
    #[serde(default)]
    pub output_amount: Option<String>,
    #[serde(default)]
    pub quote_expiry_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossBridgeRequest {
    pub deposit_id: String,
    pub signer_address: String,
    pub recipient_address: String,
    pub asset: String,
    pub amount_usd: u64,
    pub source_chain: String,
    pub destination_chain: String,
    pub quote: AcrossBridgeQuote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossBridgeAck {
    pub deposit_id: String,
    pub status: String,
    pub route_id: String,
    #[serde(default)]
    pub calldata: Option<String>,
    #[serde(default)]
    pub swap_tx: Option<SwapTx>,
    #[serde(default)]
    pub approval_txns: Option<Vec<ApprovalTx>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcrossTransferStatus {
    pub deposit_id: String,
    pub status: String,
    pub bridge_fee_usd: u64,
    pub destination_tx_id: Option<String>,
    #[serde(default)]
    pub fill_tx_hash: Option<String>,
}

#[async_trait]
pub trait AcrossTransport: Send + Sync {
    async fn quote(
        &self,
        request: AcrossBridgeQuoteRequest,
    ) -> Result<AcrossBridgeQuote, AcrossAdapterError>;

    async fn submit_bridge(
        &self,
        request: AcrossBridgeRequest,
    ) -> Result<AcrossBridgeAck, AcrossAdapterError>;

    async fn sync_status(
        &self,
        deposit_id: &str,
    ) -> Result<AcrossTransferStatus, AcrossAdapterError>;
}

#[derive(Debug, Default)]
struct NoopAcrossTransport;

#[async_trait]
impl AcrossTransport for NoopAcrossTransport {
    async fn quote(
        &self,
        _request: AcrossBridgeQuoteRequest,
    ) -> Result<AcrossBridgeQuote, AcrossAdapterError> {
        Err(AcrossAdapterError::transport(
            "across transport not configured",
        ))
    }

    async fn submit_bridge(
        &self,
        _request: AcrossBridgeRequest,
    ) -> Result<AcrossBridgeAck, AcrossAdapterError> {
        Err(AcrossAdapterError::transport(
            "across transport not configured",
        ))
    }

    async fn sync_status(
        &self,
        _deposit_id: &str,
    ) -> Result<AcrossTransferStatus, AcrossAdapterError> {
        Err(AcrossAdapterError::transport(
            "across transport not configured",
        ))
    }
}

#[derive(Clone)]
pub struct AcrossAdapter {
    transport: Arc<dyn AcrossTransport>,
    next_deposit_nonce: Arc<AtomicU64>,
}

impl Default for AcrossAdapter {
    fn default() -> Self {
        Self::with_transport(Arc::new(NoopAcrossTransport), 0)
    }
}

impl AcrossAdapter {
    pub fn with_transport(transport: Arc<dyn AcrossTransport>, seed_deposit_nonce: u64) -> Self {
        Self {
            transport,
            next_deposit_nonce: Arc::new(AtomicU64::new(seed_deposit_nonce)),
        }
    }

    pub async fn quote_bridge(
        &self,
        request: AcrossBridgeQuoteRequest,
    ) -> Result<AcrossBridgeQuote, AcrossAdapterError> {
        self.transport.quote(request).await
    }

    pub async fn sync_status(
        &self,
        deposit_id: &str,
    ) -> Result<AcrossTransferStatus, AcrossAdapterError> {
        self.transport.sync_status(deposit_id).await
    }

    pub async fn bridge_asset(
        &self,
        signer_address: &str,
        recipient_address: &str,
        request: AcrossBridgeQuoteRequest,
    ) -> Result<(AcrossBridgeAck, AcrossTransferStatus), AcrossAdapterError> {
        let quote = self.quote_bridge(request.clone()).await?;
        let deposit_id = format!(
            "across-deposit-{}",
            self.next_deposit_nonce.fetch_add(1, Ordering::SeqCst) + 1
        );
        let ack = self
            .transport
            .submit_bridge(AcrossBridgeRequest {
                deposit_id: deposit_id.clone(),
                signer_address: signer_address.to_owned(),
                recipient_address: recipient_address.to_owned(),
                asset: request.asset,
                amount_usd: request.amount_usd,
                source_chain: request.source_chain,
                destination_chain: request.destination_chain,
                quote: quote.clone(),
            })
            .await?;
        let status = self.transport.sync_status(&deposit_id).await?;
        Ok((ack, status))
    }
}
