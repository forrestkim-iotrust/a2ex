pub mod signing;
pub mod transport;

pub use transport::HyperliquidHttpTransport;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HyperliquidAdapterError {
    #[error("hyperliquid transport error: {message}")]
    Transport { message: String },
}

impl HyperliquidAdapterError {
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidOrderCommand {
    pub signer_address: String,
    pub account_address: String,
    pub asset: u32,
    pub is_buy: bool,
    pub price: String,
    pub size: String,
    pub reduce_only: bool,
    pub client_order_id: Option<String>,
    pub time_in_force: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidModifyCommand {
    pub signer_address: String,
    pub account_address: String,
    pub order_id: u64,
    pub asset: u32,
    pub is_buy: bool,
    pub price: String,
    pub size: String,
    pub reduce_only: bool,
    pub client_order_id: Option<String>,
    pub time_in_force: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidCancelCommand {
    pub signer_address: String,
    pub account_address: String,
    pub order_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidPlacedOrder {
    pub asset: u32,
    pub is_buy: bool,
    pub price: String,
    pub size: String,
    pub reduce_only: bool,
    pub client_order_id: Option<String>,
    pub time_in_force: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidModifiedOrder {
    pub order_id: u64,
    pub asset: u32,
    pub is_buy: bool,
    pub price: String,
    pub size: String,
    pub reduce_only: bool,
    pub client_order_id: Option<String>,
    pub time_in_force: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidCancelledOrder {
    pub order_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidPlaceRequest {
    pub signer_address: String,
    pub account_address: String,
    pub nonce: u64,
    pub orders: Vec<HyperliquidPlacedOrder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidModifyRequest {
    pub signer_address: String,
    pub account_address: String,
    pub nonce: u64,
    pub modifies: Vec<HyperliquidModifiedOrder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidCancelRequest {
    pub signer_address: String,
    pub account_address: String,
    pub nonce: u64,
    pub cancels: Vec<HyperliquidCancelledOrder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HyperliquidExchangeRequest {
    Place(HyperliquidPlaceRequest),
    Modify(HyperliquidModifyRequest),
    Cancel(HyperliquidCancelRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HyperliquidInfoRequest {
    OpenOrders {
        account_address: String,
    },
    OrderStatus {
        account_address: String,
        order_id: u64,
    },
    UserFills {
        account_address: String,
        aggregate_by_time: bool,
    },
    ClearinghouseState {
        account_address: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidOpenOrder {
    pub order_id: u64,
    pub asset: u32,
    pub instrument: String,
    pub is_buy: bool,
    pub price: String,
    pub size: String,
    pub reduce_only: bool,
    pub status: String,
    pub client_order_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidOrderStatus {
    pub order_id: u64,
    pub status: String,
    pub filled_size: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidUserFill {
    pub order_id: u64,
    pub asset: u32,
    pub instrument: String,
    pub size: String,
    pub price: String,
    pub side: String,
    pub filled_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidPosition {
    pub asset: u32,
    pub instrument: String,
    pub size: String,
    pub entry_price: String,
    pub position_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidClearinghouseState {
    pub positions: Vec<HyperliquidPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HyperliquidInfoResponse {
    OpenOrders(Vec<HyperliquidOpenOrder>),
    OrderStatus(HyperliquidOrderStatus),
    UserFills(Vec<HyperliquidUserFill>),
    ClearinghouseState(HyperliquidClearinghouseState),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidSyncRequest {
    pub signer_address: String,
    pub account_address: String,
    pub order_id: Option<u64>,
    pub aggregate_fills: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidSyncSnapshot {
    pub queried_account: String,
    pub queried_signer: String,
    pub open_orders: Vec<HyperliquidOpenOrder>,
    pub order_status: Option<HyperliquidOrderStatus>,
    pub fills: Vec<HyperliquidUserFill>,
    pub positions: Vec<HyperliquidPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidOrderAck {
    pub signer_address: String,
    pub account_address: String,
    pub nonce: u64,
    pub status: String,
    pub order_id: Option<u64>,
    pub client_order_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidCancelAck {
    pub signer_address: String,
    pub account_address: String,
    pub nonce: u64,
    pub status: String,
    pub order_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HyperliquidExchangeResponse {
    Order(HyperliquidOrderAck),
    Cancel(HyperliquidCancelAck),
}

#[async_trait]
pub trait HyperliquidTransport: Send + Sync {
    async fn submit_exchange(
        &self,
        request: HyperliquidExchangeRequest,
    ) -> Result<HyperliquidExchangeResponse, HyperliquidAdapterError>;

    async fn query_info(
        &self,
        request: HyperliquidInfoRequest,
    ) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError>;

    async fn withdraw(
        &self,
        _amount: &str,
        _destination: &str,
        _signer_address: &str,
    ) -> Result<String, HyperliquidAdapterError> {
        Err(HyperliquidAdapterError::transport("withdraw not supported by this transport"))
    }
}

#[derive(Debug, Default)]
struct NoopHyperliquidTransport;

#[async_trait]
impl HyperliquidTransport for NoopHyperliquidTransport {
    async fn submit_exchange(
        &self,
        _request: HyperliquidExchangeRequest,
    ) -> Result<HyperliquidExchangeResponse, HyperliquidAdapterError> {
        Err(HyperliquidAdapterError::transport(
            "hyperliquid transport not configured",
        ))
    }

    async fn query_info(
        &self,
        _request: HyperliquidInfoRequest,
    ) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError> {
        Err(HyperliquidAdapterError::transport(
            "hyperliquid transport not configured",
        ))
    }
}

#[derive(Clone)]
pub struct HyperliquidAdapter {
    transport: Arc<dyn HyperliquidTransport>,
    next_nonce: Arc<AtomicU64>,
}

impl Default for HyperliquidAdapter {
    fn default() -> Self {
        Self::with_transport(Arc::new(NoopHyperliquidTransport), 0)
    }
}

impl HyperliquidAdapter {
    pub fn transport(&self) -> &dyn HyperliquidTransport {
        self.transport.as_ref()
    }

    pub fn with_transport(transport: Arc<dyn HyperliquidTransport>, seed_nonce: u64) -> Self {
        Self {
            transport,
            next_nonce: Arc::new(AtomicU64::new(seed_nonce)),
        }
    }

    pub async fn place_order(
        &self,
        command: HyperliquidOrderCommand,
    ) -> Result<HyperliquidOrderAck, HyperliquidAdapterError> {
        let request = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: command.signer_address,
            account_address: command.account_address,
            nonce: self.allocate_nonce(),
            orders: vec![HyperliquidPlacedOrder {
                asset: command.asset,
                is_buy: command.is_buy,
                price: command.price,
                size: command.size,
                reduce_only: command.reduce_only,
                client_order_id: command.client_order_id,
                time_in_force: command.time_in_force,
            }],
        });
        match self.transport.submit_exchange(request).await? {
            HyperliquidExchangeResponse::Order(ack) => Ok(ack),
            HyperliquidExchangeResponse::Cancel(_) => Err(HyperliquidAdapterError::transport(
                "place order returned cancel acknowledgement",
            )),
        }
    }

    pub async fn modify_order(
        &self,
        command: HyperliquidModifyCommand,
    ) -> Result<HyperliquidOrderAck, HyperliquidAdapterError> {
        let request = HyperliquidExchangeRequest::Modify(HyperliquidModifyRequest {
            signer_address: command.signer_address,
            account_address: command.account_address,
            nonce: self.allocate_nonce(),
            modifies: vec![HyperliquidModifiedOrder {
                order_id: command.order_id,
                asset: command.asset,
                is_buy: command.is_buy,
                price: command.price,
                size: command.size,
                reduce_only: command.reduce_only,
                client_order_id: command.client_order_id,
                time_in_force: command.time_in_force,
            }],
        });
        match self.transport.submit_exchange(request).await? {
            HyperliquidExchangeResponse::Order(ack) => Ok(ack),
            HyperliquidExchangeResponse::Cancel(_) => Err(HyperliquidAdapterError::transport(
                "modify order returned cancel acknowledgement",
            )),
        }
    }

    pub async fn cancel_order(
        &self,
        command: HyperliquidCancelCommand,
    ) -> Result<HyperliquidCancelAck, HyperliquidAdapterError> {
        let request = HyperliquidExchangeRequest::Cancel(HyperliquidCancelRequest {
            signer_address: command.signer_address,
            account_address: command.account_address,
            nonce: self.allocate_nonce(),
            cancels: vec![HyperliquidCancelledOrder {
                order_id: command.order_id,
            }],
        });
        match self.transport.submit_exchange(request).await? {
            HyperliquidExchangeResponse::Cancel(ack) => Ok(ack),
            HyperliquidExchangeResponse::Order(_) => Err(HyperliquidAdapterError::transport(
                "cancel order returned order acknowledgement",
            )),
        }
    }

    pub async fn sync_state(
        &self,
        request: HyperliquidSyncRequest,
    ) -> Result<HyperliquidSyncSnapshot, HyperliquidAdapterError> {
        let open_orders = match self
            .transport
            .query_info(HyperliquidInfoRequest::OpenOrders {
                account_address: request.account_address.clone(),
            })
            .await?
        {
            HyperliquidInfoResponse::OpenOrders(orders) => orders,
            _ => {
                return Err(HyperliquidAdapterError::transport(
                    "openOrders response missing",
                ));
            }
        };

        let order_status = if let Some(order_id) = request.order_id {
            match self
                .transport
                .query_info(HyperliquidInfoRequest::OrderStatus {
                    account_address: request.account_address.clone(),
                    order_id,
                })
                .await?
            {
                HyperliquidInfoResponse::OrderStatus(status) => Some(status),
                _ => {
                    return Err(HyperliquidAdapterError::transport(
                        "orderStatus response missing",
                    ));
                }
            }
        } else {
            None
        };

        let fills = match self
            .transport
            .query_info(HyperliquidInfoRequest::UserFills {
                account_address: request.account_address.clone(),
                aggregate_by_time: request.aggregate_fills,
            })
            .await?
        {
            HyperliquidInfoResponse::UserFills(fills) => fills,
            _ => {
                return Err(HyperliquidAdapterError::transport(
                    "userFills response missing",
                ));
            }
        };

        let positions = match self
            .transport
            .query_info(HyperliquidInfoRequest::ClearinghouseState {
                account_address: request.account_address.clone(),
            })
            .await?
        {
            HyperliquidInfoResponse::ClearinghouseState(state) => state.positions,
            _ => {
                return Err(HyperliquidAdapterError::transport(
                    "clearinghouseState response missing",
                ));
            }
        };

        Ok(HyperliquidSyncSnapshot {
            queried_account: request.account_address,
            queried_signer: request.signer_address,
            open_orders,
            order_status,
            fills,
            positions,
        })
    }

    pub async fn place_hedge_order(
        &self,
        request: HyperliquidHedgeSubmitRequest,
    ) -> Result<HyperliquidOrderAck, HyperliquidAdapterError> {
        let prepared = request.prepared;
        let exchange_request = HyperliquidExchangeRequest::Place(HyperliquidPlaceRequest {
            signer_address: request.signer_address,
            account_address: request.account_address,
            nonce: prepared.nonce,
            orders: vec![HyperliquidPlacedOrder {
                asset: request.asset,
                is_buy: request.is_buy,
                price: request.price,
                size: request.size,
                reduce_only: prepared.reduce_only,
                client_order_id: Some(prepared.client_order_id),
                time_in_force: request.time_in_force,
            }],
        });
        match self.transport.submit_exchange(exchange_request).await? {
            HyperliquidExchangeResponse::Order(ack) => Ok(ack),
            HyperliquidExchangeResponse::Cancel(_) => Err(HyperliquidAdapterError::transport(
                "place hedge order returned cancel acknowledgement",
            )),
        }
    }

    pub fn prepare_order(
        &self,
        previous_nonce: Option<u64>,
        request: HedgeOrderRequest,
    ) -> PreparedHyperliquidOrder {
        let nonce = previous_nonce.map_or_else(|| self.allocate_nonce(), |nonce| nonce + 1);
        let client_order_id = format!("hl-{}-{}", request.strategy_id, nonce);
        PreparedHyperliquidOrder {
            venue: "hyperliquid".to_owned(),
            instrument: request.instrument,
            client_order_id: client_order_id.clone(),
            nonce,
            reduce_only: request.reduce_only,
        }
    }

    fn allocate_nonce(&self) -> u64 {
        // Hyperliquid requires nonce to be a recent timestamp in milliseconds
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        // Ensure monotonically increasing
        let prev = self.next_nonce.fetch_max(ts_ms, Ordering::SeqCst);
        if prev >= ts_ms {
            self.next_nonce.fetch_add(1, Ordering::SeqCst)
        } else {
            ts_ms
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HedgeOrderRequest {
    pub strategy_id: String,
    pub instrument: String,
    pub notional_usd: u64,
    pub reduce_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedHyperliquidOrder {
    pub venue: String,
    pub instrument: String,
    pub client_order_id: String,
    pub nonce: u64,
    pub reduce_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidHedgeSubmitRequest {
    pub prepared: PreparedHyperliquidOrder,
    pub signer_address: String,
    pub account_address: String,
    pub asset: u32,
    pub is_buy: bool,
    pub price: String,
    pub size: String,
    pub time_in_force: String,
}
