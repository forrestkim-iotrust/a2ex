use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use a2ex_across_adapter::{
    AcrossAdapterError, AcrossBridgeAck, AcrossBridgeQuote, AcrossBridgeQuoteRequest,
    AcrossBridgeRequest, AcrossTransferStatus, AcrossTransport,
};
use async_trait::async_trait;

#[derive(Debug)]
struct FakeAcrossState {
    quotes: Vec<AcrossBridgeQuoteRequest>,
    submits: Vec<AcrossBridgeRequest>,
    syncs: Vec<String>,
    submit_failures: VecDeque<String>,
}

#[derive(Debug, Clone)]
pub struct FakeAcrossTransport {
    state: Arc<Mutex<FakeAcrossState>>,
}

impl Default for FakeAcrossTransport {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeAcrossState {
                quotes: Vec::new(),
                submits: Vec::new(),
                syncs: Vec::new(),
                submit_failures: VecDeque::new(),
            })),
        }
    }
}

impl FakeAcrossTransport {
    pub fn transport(&self) -> Arc<Self> {
        Arc::new(self.clone())
    }

    pub fn quotes(&self) -> Vec<AcrossBridgeQuoteRequest> {
        self.state
            .lock()
            .expect("across quotes lock")
            .quotes
            .clone()
    }

    pub fn submits(&self) -> Vec<AcrossBridgeRequest> {
        self.state
            .lock()
            .expect("across submits lock")
            .submits
            .clone()
    }

    pub fn syncs(&self) -> Vec<String> {
        self.state.lock().expect("across syncs lock").syncs.clone()
    }

    pub fn fail_next_submit(&self, message: &str) {
        self.state
            .lock()
            .expect("across failures lock")
            .submit_failures
            .push_back(message.to_owned());
    }
}

#[async_trait]
impl AcrossTransport for FakeAcrossTransport {
    async fn quote(
        &self,
        request: AcrossBridgeQuoteRequest,
    ) -> Result<AcrossBridgeQuote, AcrossAdapterError> {
        self.state
            .lock()
            .expect("across quotes lock")
            .quotes
            .push(request.clone());
        Ok(AcrossBridgeQuote {
            route_id: format!(
                "route-{}-{}",
                request.source_chain, request.destination_chain
            ),
            bridge_fee_usd: 7,
            expected_fill_seconds: 45,
            approval: a2ex_across_adapter::AcrossApproval {
                token: request.asset,
                spender: "0xacross-spender".to_owned(),
                allowance_target: "0xacross-spender".to_owned(),
            },
            calldata: None,
            swap_tx: None,
            input_amount: None,
            output_amount: None,
            quote_expiry_secs: None,
        })
    }

    async fn submit_bridge(
        &self,
        request: AcrossBridgeRequest,
    ) -> Result<AcrossBridgeAck, AcrossAdapterError> {
        let mut state = self.state.lock().expect("across submit lock");
        state.submits.push(request.clone());
        if let Some(message) = state.submit_failures.pop_front() {
            return Err(AcrossAdapterError::transport(message));
        }
        Ok(AcrossBridgeAck {
            deposit_id: request.deposit_id,
            status: "submitted".to_owned(),
            route_id: request.quote.route_id,
            calldata: None,
            swap_tx: None,
            approval_txns: None,
        })
    }

    async fn sync_status(
        &self,
        deposit_id: &str,
    ) -> Result<AcrossTransferStatus, AcrossAdapterError> {
        self.state
            .lock()
            .expect("across sync lock")
            .syncs
            .push(deposit_id.to_owned());
        Ok(AcrossTransferStatus {
            deposit_id: deposit_id.to_owned(),
            status: "settled".to_owned(),
            bridge_fee_usd: 7,
            destination_tx_id: Some(format!("dest-{deposit_id}")),
            fill_tx_hash: None,
        })
    }
}
