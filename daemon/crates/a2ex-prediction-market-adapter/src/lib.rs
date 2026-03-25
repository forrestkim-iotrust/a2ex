pub mod signing;
pub mod transport;

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PredictionMarketAdapterError {
    #[error("prediction-market transport error: {message}")]
    Transport { message: String },
}

impl PredictionMarketAdapterError {
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionVenue {
    Polymarket,
    Kalshi,
}

impl PredictionVenue {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Polymarket => "polymarket",
            Self::Kalshi => "kalshi",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionAuth {
    pub credential_id: String,
    pub auth_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionOrderRequest {
    pub venue: PredictionVenue,
    pub market: String,
    pub side: String,
    pub size: String,
    pub price: String,
    pub max_fee_bps: u64,
    pub max_slippage_bps: u64,
    pub idempotency_key: String,
    pub auth: PredictionAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionOrderAck {
    pub venue: PredictionVenue,
    pub order_id: String,
    pub status: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionOrderStatus {
    pub venue: PredictionVenue,
    pub order_id: String,
    pub status: String,
    pub filled_amount: String,
}

#[async_trait]
pub trait PredictionMarketTransport: Send + Sync {
    async fn place_order(
        &self,
        request: PredictionOrderRequest,
    ) -> Result<PredictionOrderAck, PredictionMarketAdapterError>;

    async fn sync_order(
        &self,
        venue: PredictionVenue,
        order_id: &str,
    ) -> Result<PredictionOrderStatus, PredictionMarketAdapterError>;
}

#[derive(Debug, Default)]
struct NoopPredictionTransport;

#[async_trait]
impl PredictionMarketTransport for NoopPredictionTransport {
    async fn place_order(
        &self,
        _request: PredictionOrderRequest,
    ) -> Result<PredictionOrderAck, PredictionMarketAdapterError> {
        Err(PredictionMarketAdapterError::transport(
            "prediction-market transport not configured",
        ))
    }

    async fn sync_order(
        &self,
        _venue: PredictionVenue,
        _order_id: &str,
    ) -> Result<PredictionOrderStatus, PredictionMarketAdapterError> {
        Err(PredictionMarketAdapterError::transport(
            "prediction-market transport not configured",
        ))
    }
}

#[derive(Clone)]
pub struct PredictionMarketAdapter {
    transport: Arc<RwLock<Arc<dyn PredictionMarketTransport>>>,
}

impl Default for PredictionMarketAdapter {
    fn default() -> Self {
        Self::with_transport(Arc::new(NoopPredictionTransport))
    }
}

impl PredictionMarketAdapter {
    pub fn with_transport(transport: Arc<dyn PredictionMarketTransport>) -> Self {
        Self {
            transport: Arc::new(RwLock::new(transport)),
        }
    }

    /// Replace the underlying transport (e.g. after credentials are derived).
    pub fn set_transport(&self, transport: Arc<dyn PredictionMarketTransport>) {
        let mut guard = self.transport.write().expect("prediction market transport write lock");
        *guard = transport;
    }

    pub async fn place_and_sync(
        &self,
        request: PredictionOrderRequest,
    ) -> Result<(PredictionOrderAck, PredictionOrderStatus), PredictionMarketAdapterError> {
        let transport = {
            let guard = self.transport.read().expect("prediction market transport read lock");
            Arc::clone(&*guard)
        };
        let ack = transport.place_order(request).await?;
        let status = transport.sync_order(ack.venue, &ack.order_id).await?;
        Ok((ack, status))
    }
}

// Re-exports for convenience
pub use signing::PolymarketApiCredentials;
pub use transport::PolymarketHttpTransport;
