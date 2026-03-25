use std::sync::{Arc, Mutex};

use a2ex_hyperliquid_adapter::{
    HyperliquidAdapterError, HyperliquidCancelAck, HyperliquidClearinghouseState,
    HyperliquidExchangeRequest, HyperliquidExchangeResponse, HyperliquidInfoRequest,
    HyperliquidInfoResponse, HyperliquidOpenOrder, HyperliquidOrderAck, HyperliquidOrderStatus,
    HyperliquidPosition, HyperliquidTransport, HyperliquidUserFill,
};
use async_trait::async_trait;

#[derive(Debug, Default)]
struct FakeHyperliquidState {
    exchange_requests: Vec<HyperliquidExchangeRequest>,
    info_requests: Vec<HyperliquidInfoRequest>,
    open_orders: Vec<HyperliquidOpenOrder>,
    order_status: Option<HyperliquidOrderStatus>,
    user_fills: Vec<HyperliquidUserFill>,
    positions: Vec<HyperliquidPosition>,
}

#[derive(Debug, Clone, Default)]
pub struct FakeHyperliquidTransport {
    state: Arc<Mutex<FakeHyperliquidState>>,
}

impl FakeHyperliquidTransport {
    pub fn transport(&self) -> Arc<Self> {
        Arc::new(self.clone())
    }

    pub fn exchange_requests(&self) -> Vec<HyperliquidExchangeRequest> {
        self.state
            .lock()
            .expect("fake hyperliquid exchange request lock")
            .exchange_requests
            .clone()
    }

    pub fn info_requests(&self) -> Vec<HyperliquidInfoRequest> {
        self.state
            .lock()
            .expect("fake hyperliquid info request lock")
            .info_requests
            .clone()
    }

    pub fn seed_open_orders(&self, open_orders: Vec<HyperliquidOpenOrder>) {
        self.state
            .lock()
            .expect("fake hyperliquid open order seed lock")
            .open_orders = open_orders;
    }

    pub fn seed_order_status(&self, order_status: HyperliquidOrderStatus) {
        self.state
            .lock()
            .expect("fake hyperliquid order status seed lock")
            .order_status = Some(order_status);
    }

    pub fn seed_user_fills(&self, user_fills: Vec<HyperliquidUserFill>) {
        self.state
            .lock()
            .expect("fake hyperliquid user fills seed lock")
            .user_fills = user_fills;
    }

    pub fn seed_positions(&self, positions: Vec<HyperliquidPosition>) {
        self.state
            .lock()
            .expect("fake hyperliquid positions seed lock")
            .positions = positions;
    }
}

#[async_trait]
impl HyperliquidTransport for FakeHyperliquidTransport {
    async fn submit_exchange(
        &self,
        request: HyperliquidExchangeRequest,
    ) -> Result<HyperliquidExchangeResponse, HyperliquidAdapterError> {
        self.state
            .lock()
            .expect("fake hyperliquid exchange request lock")
            .exchange_requests
            .push(request.clone());

        Ok(match request {
            HyperliquidExchangeRequest::Place(place) => {
                HyperliquidExchangeResponse::Order(HyperliquidOrderAck {
                    signer_address: place.signer_address,
                    account_address: place.account_address,
                    nonce: place.nonce,
                    status: "resting".to_owned(),
                    order_id: Some(91),
                    client_order_id: place
                        .orders
                        .first()
                        .and_then(|order| order.client_order_id.clone()),
                })
            }
            HyperliquidExchangeRequest::Modify(modify) => {
                HyperliquidExchangeResponse::Order(HyperliquidOrderAck {
                    signer_address: modify.signer_address,
                    account_address: modify.account_address,
                    nonce: modify.nonce,
                    status: "modified".to_owned(),
                    order_id: Some(modify.modifies[0].order_id),
                    client_order_id: modify
                        .modifies
                        .first()
                        .and_then(|entry| entry.client_order_id.clone()),
                })
            }
            HyperliquidExchangeRequest::Cancel(cancel) => {
                HyperliquidExchangeResponse::Cancel(HyperliquidCancelAck {
                    signer_address: cancel.signer_address,
                    account_address: cancel.account_address,
                    nonce: cancel.nonce,
                    status: "cancelled".to_owned(),
                    order_id: cancel.cancels[0].order_id,
                })
            }
        })
    }

    async fn query_info(
        &self,
        request: HyperliquidInfoRequest,
    ) -> Result<HyperliquidInfoResponse, HyperliquidAdapterError> {
        let mut state = self
            .state
            .lock()
            .expect("fake hyperliquid info request lock");
        state.info_requests.push(request.clone());

        match request {
            HyperliquidInfoRequest::OpenOrders { .. } => Ok(HyperliquidInfoResponse::OpenOrders(
                state.open_orders.clone(),
            )),
            HyperliquidInfoRequest::OrderStatus { .. } => state
                .order_status
                .clone()
                .map(HyperliquidInfoResponse::OrderStatus)
                .ok_or_else(|| HyperliquidAdapterError::transport("order status not configured")),
            HyperliquidInfoRequest::UserFills { .. } => {
                Ok(HyperliquidInfoResponse::UserFills(state.user_fills.clone()))
            }
            HyperliquidInfoRequest::ClearinghouseState { .. } => Ok(
                HyperliquidInfoResponse::ClearinghouseState(HyperliquidClearinghouseState {
                    positions: state.positions.clone(),
                }),
            ),
        }
    }
}
