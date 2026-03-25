use std::time::Duration;

use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedEvmTransaction {
    pub chain_id: u64,
    pub to: String,
    pub value_wei: String,
    pub calldata: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTransactionBytes {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxLifecycleStatus {
    Prepared,
    Submitted,
    Pending,
    Confirmed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxReceiptMetadata {
    pub chain_id: u64,
    pub tx_hash: String,
    pub confirmation_depth: u64,
    pub block_number: Option<u64>,
    pub receipt_status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxLifecycleEvent {
    pub status: TxLifecycleStatus,
    pub metadata: TxReceiptMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxLifecycleReport {
    pub prepared: PreparedEvmTransaction,
    pub events: Vec<TxLifecycleEvent>,
}

impl TxLifecycleReport {
    pub fn terminal_status(&self) -> Option<&TxLifecycleStatus> {
        self.events.last().map(|event| &event.status)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EvmAdapterError {
    #[error("signed transaction bytes are empty")]
    EmptySignedPayload,
    #[error("provider url is invalid: {message}")]
    InvalidProviderUrl { message: String },
    #[error("adapter failed to submit transaction: {message}")]
    Submission { message: String },
}

#[async_trait]
pub trait EvmAdapter: Send + Sync {
    async fn submit_and_watch(
        &self,
        prepared: PreparedEvmTransaction,
        signed: SignedTransactionBytes,
    ) -> Result<TxLifecycleReport, EvmAdapterError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEvmAdapter;

#[derive(Debug, Clone)]
pub struct ProviderBackedEvmAdapter {
    provider: DynProvider,
    required_confirmations: u64,
    receipt_timeout: Duration,
}

impl ProviderBackedEvmAdapter {
    pub fn new(rpc_url: impl AsRef<str>) -> Result<Self, EvmAdapterError> {
        let url = rpc_url.as_ref().parse::<reqwest::Url>().map_err(|error| {
            EvmAdapterError::InvalidProviderUrl {
                message: error.to_string(),
            }
        })?;
        Ok(Self::from_http_url(url))
    }

    pub fn from_http_url(url: reqwest::Url) -> Self {
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_http(url)
            .erased();
        Self::from_provider(provider)
    }

    pub fn from_provider(provider: DynProvider) -> Self {
        Self {
            provider,
            required_confirmations: 1,
            receipt_timeout: Duration::from_secs(10),
        }
    }

    pub fn with_required_confirmations(mut self, required_confirmations: u64) -> Self {
        self.required_confirmations = required_confirmations.max(1);
        self
    }

    pub fn with_receipt_timeout(mut self, receipt_timeout: Duration) -> Self {
        self.receipt_timeout = receipt_timeout;
        self
    }
}

#[async_trait]
impl EvmAdapter for NoopEvmAdapter {
    async fn submit_and_watch(
        &self,
        _prepared: PreparedEvmTransaction,
        _signed: SignedTransactionBytes,
    ) -> Result<TxLifecycleReport, EvmAdapterError> {
        Err(EvmAdapterError::Submission {
            message: "explicit adapter required".to_owned(),
        })
    }
}

#[async_trait]
impl EvmAdapter for ProviderBackedEvmAdapter {
    async fn submit_and_watch(
        &self,
        prepared: PreparedEvmTransaction,
        signed: SignedTransactionBytes,
    ) -> Result<TxLifecycleReport, EvmAdapterError> {
        if signed.bytes.is_empty() {
            return Err(EvmAdapterError::EmptySignedPayload);
        }

        let pending = self
            .provider
            .send_raw_transaction(&signed.bytes)
            .await
            .map_err(|error| EvmAdapterError::Submission {
                message: error.to_string(),
            })?;
        let tx_hash = pending.tx_hash().to_string();
        let mut events = vec![
            submitted_event(prepared.chain_id, tx_hash.clone()),
            pending_event(prepared.chain_id, tx_hash.clone()),
        ];

        let receipt = pending
            .with_required_confirmations(self.required_confirmations)
            .with_timeout(Some(self.receipt_timeout))
            .get_receipt()
            .await;

        match receipt {
            Ok(receipt) => {
                let metadata = receipt_metadata_from_value(
                    prepared.chain_id,
                    tx_hash,
                    self.required_confirmations,
                    serde_json::to_value(&receipt).unwrap_or(Value::Null),
                );
                let status =
                    if metadata.receipt_status == "0x1" || metadata.receipt_status == "confirmed" {
                        TxLifecycleStatus::Confirmed
                    } else {
                        TxLifecycleStatus::Failed
                    };
                events.push(TxLifecycleEvent { status, metadata });
            }
            Err(error) => {
                events.push(TxLifecycleEvent {
                    status: TxLifecycleStatus::Failed,
                    metadata: TxReceiptMetadata {
                        chain_id: prepared.chain_id,
                        tx_hash,
                        confirmation_depth: 0,
                        block_number: None,
                        receipt_status: "failed".to_owned(),
                        error: Some(error.to_string()),
                    },
                });
            }
        }

        Ok(TxLifecycleReport { prepared, events })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimulatedOutcome {
    Confirmed,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedEvmAdapter {
    pub block_number: u64,
    pub confirmation_depth: u64,
    pub outcome: SimulatedOutcome,
}

#[async_trait]
impl EvmAdapter for SimulatedEvmAdapter {
    async fn submit_and_watch(
        &self,
        prepared: PreparedEvmTransaction,
        signed: SignedTransactionBytes,
    ) -> Result<TxLifecycleReport, EvmAdapterError> {
        if signed.bytes.is_empty() {
            return Err(EvmAdapterError::EmptySignedPayload);
        }

        let tx_hash = transaction_hash(&signed.bytes);
        let submitted = TxLifecycleEvent {
            status: TxLifecycleStatus::Submitted,
            metadata: TxReceiptMetadata {
                chain_id: prepared.chain_id,
                tx_hash: tx_hash.clone(),
                confirmation_depth: 0,
                block_number: None,
                receipt_status: "submitted".to_owned(),
                error: None,
            },
        };
        let pending = TxLifecycleEvent {
            status: TxLifecycleStatus::Pending,
            metadata: TxReceiptMetadata {
                chain_id: prepared.chain_id,
                tx_hash: tx_hash.clone(),
                confirmation_depth: 0,
                block_number: Some(self.block_number),
                receipt_status: "pending".to_owned(),
                error: None,
            },
        };
        let terminal = match &self.outcome {
            SimulatedOutcome::Confirmed => TxLifecycleEvent {
                status: TxLifecycleStatus::Confirmed,
                metadata: TxReceiptMetadata {
                    chain_id: prepared.chain_id,
                    tx_hash,
                    confirmation_depth: self.confirmation_depth,
                    block_number: Some(self.block_number),
                    receipt_status: "confirmed".to_owned(),
                    error: None,
                },
            },
            SimulatedOutcome::Failed { reason } => TxLifecycleEvent {
                status: TxLifecycleStatus::Failed,
                metadata: TxReceiptMetadata {
                    chain_id: prepared.chain_id,
                    tx_hash,
                    confirmation_depth: 0,
                    block_number: Some(self.block_number),
                    receipt_status: "failed".to_owned(),
                    error: Some(reason.clone()),
                },
            },
        };

        Ok(TxLifecycleReport {
            prepared,
            events: vec![submitted, pending, terminal],
        })
    }
}

fn transaction_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("0x{}", hex::encode(hasher.finalize()))
}

fn submitted_event(chain_id: u64, tx_hash: String) -> TxLifecycleEvent {
    TxLifecycleEvent {
        status: TxLifecycleStatus::Submitted,
        metadata: TxReceiptMetadata {
            chain_id,
            tx_hash,
            confirmation_depth: 0,
            block_number: None,
            receipt_status: "submitted".to_owned(),
            error: None,
        },
    }
}

fn pending_event(chain_id: u64, tx_hash: String) -> TxLifecycleEvent {
    TxLifecycleEvent {
        status: TxLifecycleStatus::Pending,
        metadata: TxReceiptMetadata {
            chain_id,
            tx_hash,
            confirmation_depth: 0,
            block_number: None,
            receipt_status: "pending".to_owned(),
            error: None,
        },
    }
}

fn receipt_metadata_from_value(
    chain_id: u64,
    fallback_tx_hash: String,
    confirmation_depth: u64,
    receipt: Value,
) -> TxReceiptMetadata {
    let tx_hash = read_string_field(&receipt, "transactionHash").unwrap_or(fallback_tx_hash);
    let block_number = read_u64_field(&receipt, "blockNumber");
    let status_raw =
        read_string_field(&receipt, "status").unwrap_or_else(|| "confirmed".to_owned());
    let is_success = matches!(status_raw.as_str(), "0x1" | "0x01" | "1" | "confirmed");

    TxReceiptMetadata {
        chain_id,
        tx_hash,
        confirmation_depth: if block_number.is_some() {
            confirmation_depth
        } else {
            0
        },
        block_number,
        receipt_status: if is_success {
            "confirmed".to_owned()
        } else {
            status_raw
        },
        error: if is_success {
            None
        } else {
            Some("transaction receipt status indicates failure".to_owned())
        },
    }
}

fn read_string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn read_u64_field(value: &Value, field: &str) -> Option<u64> {
    let raw = value.get(field)?;
    match raw {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => parse_u64(text),
        _ => None,
    }
}

fn parse_u64(value: &str) -> Option<u64> {
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
