use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::{
    AcrossAdapterError, AcrossApproval, AcrossBridgeAck, AcrossBridgeQuote,
    AcrossBridgeQuoteRequest, AcrossBridgeRequest, AcrossTransferStatus, AcrossTransport,
    ApprovalTx, SwapTx,
};

// ---------------------------------------------------------------------------
// Swap API response serde structs (matches real Across /swap/approval response)
// ---------------------------------------------------------------------------

/// Top-level swap transaction returned at the root of the response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwapApiSwapTx {
    to: String,
    data: String,
    #[serde(default)]
    chain_id: Option<u64>,
    #[serde(default)]
    gas: Option<String>,
}

/// Approval transaction returned at the root of the response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwapApiApprovalTx {
    to: String,
    data: String,
    #[serde(default)]
    chain_id: Option<u64>,
}

/// Token info.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SwapApiToken {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    decimals: Option<u32>,
    #[serde(default)]
    chain_id: Option<u64>,
}

/// Fee amount info.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SwapApiFeeAmount {
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    amount_usd: Option<String>,
    #[serde(default)]
    pct: Option<String>,
    #[serde(default)]
    token: Option<SwapApiToken>,
}

/// Top-level fees object.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SwapApiFees {
    #[serde(default)]
    total: Option<SwapApiFeeAmount>,
}

/// Allowance check.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SwapApiAllowanceCheck {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    spender: Option<String>,
}

/// Checks object.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SwapApiChecks {
    #[serde(default)]
    allowance: Option<SwapApiAllowanceCheck>,
}

/// Top-level response from `GET /swap/approval`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SwapApiResponse {
    // Amounts
    #[serde(default)]
    input_amount: Option<String>,
    #[serde(default)]
    expected_output_amount: Option<String>,
    #[serde(default)]
    min_output_amount: Option<String>,

    // Fill time
    #[serde(default)]
    expected_fill_time: Option<u64>,

    // Quote expiry
    #[serde(default)]
    quote_expiry_timestamp: Option<u64>,

    // Top-level swap tx
    #[serde(default)]
    swap_tx: Option<SwapApiSwapTx>,

    // Approval transactions
    #[serde(default)]
    approval_txns: Option<Vec<SwapApiApprovalTx>>,

    // Token info
    #[serde(default)]
    input_token: Option<SwapApiToken>,
    #[serde(default)]
    output_token: Option<SwapApiToken>,

    // Fees
    #[serde(default)]
    fees: Option<SwapApiFees>,

    // Checks (contains allowance spender)
    #[serde(default)]
    checks: Option<SwapApiChecks>,
}

// ---------------------------------------------------------------------------
// Deposit Status API response serde structs
// ---------------------------------------------------------------------------

/// Response from `GET /deposit/status`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct DepositStatusResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    fill_txn_ref: Option<String>,
    #[serde(default)]
    deposit_txn_ref: Option<String>,
    #[serde(default)]
    origin_chain_id: Option<u64>,
    #[serde(default)]
    deposit_id: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Transport implementation
// ---------------------------------------------------------------------------

/// HTTP transport for the Across Swap API and Deposit Status API.
#[derive(Debug, Clone)]
pub struct AcrossHttpTransport {
    base_url: String,
    client: Client,
    integrator_id: Option<String>,
    api_key: Option<String>,
}

impl AcrossHttpTransport {
    /// Create a new transport with a 15-second request timeout.
    pub fn new(
        base_url: impl Into<String>,
        integrator_id: Option<String>,
        api_key: Option<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to build reqwest client");

        Self {
            base_url: base_url.into(),
            client,
            integrator_id,
            api_key,
        }
    }
}

#[async_trait]
impl AcrossTransport for AcrossHttpTransport {
    async fn quote(
        &self,
        request: AcrossBridgeQuoteRequest,
    ) -> Result<AcrossBridgeQuote, AcrossAdapterError> {
        let url = format!("{}/swap/approval", self.base_url);

        let depositor = request
            .depositor
            .as_deref()
            .unwrap_or("0x0000000000000000000000000000000000000000");

        let mut req = self
            .client
            .get(&url)
            .query(&[("tradeType", "exactInput")])
            .query(&[("amount", &request.amount_usd.to_string())])
            .query(&[("inputToken", &request.asset)])
            .query(&[("originChainId", &request.source_chain)])
            .query(&[("destinationChainId", &request.destination_chain)])
            .query(&[("depositor", depositor)]);

        // output_token: default to same as input (bridgeable-to-bridgeable)
        if let Some(ref output_token) = request.output_token {
            req = req.query(&[("outputToken", output_token.as_str())]);
        } else {
            req = req.query(&[("outputToken", request.asset.as_str())]);
        }

        // recipient defaults to depositor
        if let Some(ref recipient) = request.recipient {
            req = req.query(&[("recipient", recipient.as_str())]);
        } else {
            req = req.query(&[("recipient", depositor)]);
        }

        if let Some(ref integrator_id) = self.integrator_id {
            req = req.query(&[("integratorId", integrator_id)]);
        }

        if let Some(ref api_key) = self.api_key {
            req = req.bearer_auth(api_key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AcrossAdapterError::transport(format!("HTTP request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AcrossAdapterError::transport(format!(
                "HTTP {status}: {body}"
            )));
        }

        let api_resp: SwapApiResponse = resp
            .json()
            .await
            .map_err(|e| AcrossAdapterError::transport(format!("failed to parse response: {e}")))?;

        // Extract swap tx from the top-level swapTx field.
        let swap_tx = api_resp.swap_tx.as_ref().map(|tx| SwapTx {
            to: tx.to.clone(),
            data: tx.data.clone(),
            value: tx.gas.clone().unwrap_or_else(|| "0".to_string()),
        });

        let calldata = api_resp.swap_tx.as_ref().map(|tx| tx.data.clone());

        // Extract approval transactions.
        let approval_txns: Vec<ApprovalTx> = api_resp
            .approval_txns
            .as_ref()
            .map(|txns| {
                txns.iter()
                    .map(|tx| ApprovalTx {
                        to: tx.to.clone(),
                        data: tx.data.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Build approval info.
        let approval = {
            let spender = api_resp
                .checks
                .as_ref()
                .and_then(|c| c.allowance.as_ref())
                .and_then(|a| a.spender.clone())
                .or_else(|| swap_tx.as_ref().map(|s| s.to.clone()))
                .unwrap_or_default();

            let allowance_target = api_resp
                .checks
                .as_ref()
                .and_then(|c| c.allowance.as_ref())
                .and_then(|a| a.token.clone())
                .unwrap_or_default();

            let token = api_resp
                .input_token
                .as_ref()
                .and_then(|t| t.address.clone())
                .unwrap_or_default();

            AcrossApproval {
                token,
                spender,
                allowance_target,
            }
        };

        let input_amount = api_resp.input_amount.clone();
        let output_amount = api_resp
            .expected_output_amount
            .clone()
            .or(api_resp.min_output_amount.clone());

        let expected_fill_seconds = api_resp.expected_fill_time.unwrap_or(0);
        let quote_expiry_secs = api_resp.quote_expiry_timestamp;

        let bridge_fee_usd = api_resp
            .fees
            .as_ref()
            .and_then(|f| f.total.as_ref())
            .and_then(|t| t.amount.as_ref())
            .and_then(|a| a.parse::<u64>().ok())
            .unwrap_or(0);

        let route_id = format!(
            "across-{}-{}",
            request.source_chain, request.destination_chain
        );

        Ok(AcrossBridgeQuote {
            route_id,
            bridge_fee_usd,
            expected_fill_seconds,
            approval,
            approval_txns,
            calldata,
            swap_tx,
            input_amount,
            output_amount,
            quote_expiry_secs,
        })
    }

    async fn submit_bridge(
        &self,
        request: AcrossBridgeRequest,
    ) -> Result<AcrossBridgeAck, AcrossAdapterError> {
        // 와리가리 pattern: no HTTP call. Extract calldata from the embedded
        // quote and return it in the ack for OpenClaw to submit via vault.
        let quote = &request.quote;

        Ok(AcrossBridgeAck {
            deposit_id: request.deposit_id,
            status: "pending".to_string(),
            route_id: quote.route_id.clone(),
            calldata: quote.calldata.clone(),
            swap_tx: quote.swap_tx.clone(),
            approval_txns: None,
        })
    }

    async fn sync_status(
        &self,
        deposit_id: &str,
    ) -> Result<AcrossTransferStatus, AcrossAdapterError> {
        let url = format!("{}/deposit/status", self.base_url);

        let mut req = self
            .client
            .get(&url)
            .query(&[("depositTxnRef", deposit_id)]);

        if let Some(ref api_key) = self.api_key {
            req = req.bearer_auth(api_key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AcrossAdapterError::transport(format!("HTTP request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AcrossAdapterError::transport(format!(
                "HTTP {status}: {body}"
            )));
        }

        let api_resp: DepositStatusResponse = resp
            .json()
            .await
            .map_err(|e| AcrossAdapterError::transport(format!("failed to parse response: {e}")))?;

        Ok(AcrossTransferStatus {
            deposit_id: deposit_id.to_string(),
            status: api_resp.status,
            bridge_fee_usd: 0,
            fill_tx_hash: api_resp.fill_txn_ref.clone(),
            destination_tx_id: api_resp.fill_txn_ref,
        })
    }
}
