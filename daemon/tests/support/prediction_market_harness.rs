use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use a2ex_prediction_market_adapter::{
    PredictionMarketAdapterError, PredictionMarketTransport, PredictionOrderAck,
    PredictionOrderRequest, PredictionOrderStatus, PredictionVenue,
};
use async_trait::async_trait;

#[derive(Debug)]
struct FakePredictionState {
    requests: Vec<PredictionOrderRequest>,
    syncs: Vec<(PredictionVenue, String)>,
    failures: HashMap<String, VecDeque<String>>,
}

#[derive(Debug, Clone)]
pub struct FakePredictionMarketTransport {
    state: Arc<Mutex<FakePredictionState>>,
}

impl Default for FakePredictionMarketTransport {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakePredictionState {
                requests: Vec::new(),
                syncs: Vec::new(),
                failures: HashMap::new(),
            })),
        }
    }
}

impl FakePredictionMarketTransport {
    pub fn transport(&self) -> Arc<Self> {
        Arc::new(self.clone())
    }

    pub fn requests(&self) -> Vec<PredictionOrderRequest> {
        self.state
            .lock()
            .expect("prediction requests lock")
            .requests
            .clone()
    }

    pub fn syncs(&self) -> Vec<(PredictionVenue, String)> {
        self.state
            .lock()
            .expect("prediction syncs lock")
            .syncs
            .clone()
    }

    pub fn fail_next_for_venue(&self, venue: &str, message: &str) {
        self.state
            .lock()
            .expect("prediction failures lock")
            .failures
            .entry(venue.to_owned())
            .or_default()
            .push_back(message.to_owned());
    }
}

#[async_trait]
impl PredictionMarketTransport for FakePredictionMarketTransport {
    async fn place_order(
        &self,
        request: PredictionOrderRequest,
    ) -> Result<PredictionOrderAck, PredictionMarketAdapterError> {
        let mut state = self.state.lock().expect("prediction request lock");
        state.requests.push(request.clone());
        if let Some(queue) = state.failures.get_mut(request.venue.as_str())
            && let Some(message) = queue.pop_front()
        {
            return Err(PredictionMarketAdapterError::transport(message));
        }
        Ok(PredictionOrderAck {
            venue: request.venue,
            order_id: format!("{}-order-{}", request.venue.as_str(), state.requests.len()),
            status: "accepted".to_owned(),
            idempotency_key: request.idempotency_key,
        })
    }

    async fn sync_order(
        &self,
        venue: PredictionVenue,
        order_id: &str,
    ) -> Result<PredictionOrderStatus, PredictionMarketAdapterError> {
        self.state
            .lock()
            .expect("prediction sync lock")
            .syncs
            .push((venue, order_id.to_owned()));
        Ok(PredictionOrderStatus {
            venue,
            order_id: order_id.to_owned(),
            status: "filled".to_owned(),
            filled_amount: "1800".to_owned(),
        })
    }
}
