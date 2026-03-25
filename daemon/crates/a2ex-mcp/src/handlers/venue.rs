use std::sync::Arc;

use a2ex_prediction_market_adapter::PolymarketApiCredentials;

use crate::error::McpContractError;
use crate::server::{A2exSkillMcpServer, resolve_chain_id, resolve_hyperliquid_asset_index};
use crate::venue_tools::{
    BridgeStatusRequest, BridgeStatusResponse, DeriveApiKeyRequest, DeriveApiKeyResponse,
    PrepareBridgeRequest, PrepareBridgeResponse, QueryPositionsRequest, QueryPositionsResponse,
    TradeHyperliquidRequest, TradeHyperliquidResponse, TradePolymarketRequest,
    TradePolymarketResponse, TOOL_VENUE_BRIDGE_STATUS, TOOL_VENUE_DERIVE_API_KEY,
    TOOL_VENUE_PREPARE_BRIDGE, TOOL_VENUE_QUERY_POSITIONS, TOOL_VENUE_TRADE_HYPERLIQUID,
    TOOL_VENUE_TRADE_POLYMARKET,
};

impl A2exSkillMcpServer {
    pub async fn handle_prepare_bridge(
        &self,
        request: PrepareBridgeRequest,
    ) -> Result<PrepareBridgeResponse, McpContractError> {
        tracing::info!(
            tool = TOOL_VENUE_PREPARE_BRIDGE,
            asset = %request.asset,
            amount_usd = request.amount_usd,
            source_chain = %request.source_chain,
            destination_chain = %request.destination_chain,
            "venue tool entry"
        );
        let adapters = self.get_venue_adapters()?;
        let quote = adapters
            .across
            .quote_bridge(a2ex_across_adapter::AcrossBridgeQuoteRequest {
                asset: request.asset,
                amount_usd: request.amount_usd,
                source_chain: request.source_chain.clone(),
                destination_chain: request.destination_chain,
                depositor: request.depositor,
                recipient: request.recipient,
                output_token: request.output_token,
            })
            .await
            .map_err(|e| {
                tracing::error!(venue = "across", error = %e, "bridge quote failed");
                McpContractError::VenueTransport {
                    venue: "across".to_owned(),
                    message: e.to_string(),
                }
            })?;

        let chain_id = resolve_chain_id(&request.source_chain);

        let swap_tx = match &quote.swap_tx {
            Some(tx) => crate::venue_tools::SwapTxEnvelope {
                to: tx.to.clone(),
                data: tx.data.clone(),
                value: tx.value.clone(),
            },
            None => crate::venue_tools::SwapTxEnvelope {
                to: String::new(),
                data: quote.calldata.clone().unwrap_or_default(),
                value: "0".to_owned(),
            },
        };

        let approval_txns: Vec<crate::venue_tools::ApprovalTxEnvelope> = quote
            .approval_txns
            .iter()
            .map(|tx| crate::venue_tools::ApprovalTxEnvelope {
                to: tx.to.clone(),
                data: tx.data.clone(),
            })
            .collect();

        Ok(PrepareBridgeResponse {
            swap_tx,
            approval_txns,
            chain_id,
            quote: crate::venue_tools::BridgeQuoteMetadata {
                route_id: quote.route_id,
                bridge_fee_usd: quote.bridge_fee_usd,
                expected_fill_seconds: quote.expected_fill_seconds,
                input_amount: quote.input_amount,
                output_amount: quote.output_amount,
            },
            next_step: "Submit approval transactions first, then swap_tx via waiaas.call_contract, then check status via venue.bridge_status".to_owned(),
        })
    }

    /// 직통 — sign and submit a Hyperliquid order internally.
    pub async fn handle_trade_hyperliquid(
        &self,
        request: TradeHyperliquidRequest,
    ) -> Result<TradeHyperliquidResponse, McpContractError> {
        tracing::info!(
            tool = TOOL_VENUE_TRADE_HYPERLIQUID,
            asset = %request.asset,
            is_buy = request.is_buy,
            size = %request.size,
            price = %request.price,
            "venue tool entry"
        );
        let adapters = self.get_venue_adapters()?;

        // Resolve asset index from symbol (simple mapping for common assets)
        let asset_index = resolve_hyperliquid_asset_index(&request.asset);

        let ack = adapters
            .hyperliquid
            .place_order(a2ex_hyperliquid_adapter::HyperliquidOrderCommand {
                signer_address: String::new(),  // filled by transport/signer
                account_address: String::new(), // filled by transport/signer
                asset: asset_index,
                is_buy: request.is_buy,
                price: request.price,
                size: request.size,
                reduce_only: request.reduce_only,
                client_order_id: None,
                time_in_force: if request.order_type == "market" {
                    "Ioc".to_owned()
                } else {
                    "Gtc".to_owned()
                },
            })
            .await
            .map_err(|e| {
                tracing::error!(venue = "hyperliquid", error = %e, "order placement failed");
                McpContractError::VenueTransport {
                    venue: "hyperliquid".to_owned(),
                    message: e.to_string(),
                }
            })?;

        Ok(TradeHyperliquidResponse {
            order_id: ack.order_id.map(|id| id.to_string()).unwrap_or_default(),
            status: ack.status,
            venue: "hyperliquid".to_owned(),
        })
    }

    /// 직통 — sign and submit a Polymarket order internally.
    pub async fn handle_trade_polymarket(
        &self,
        request: TradePolymarketRequest,
    ) -> Result<TradePolymarketResponse, McpContractError> {
        tracing::info!(
            tool = TOOL_VENUE_TRADE_POLYMARKET,
            token_id = %request.token_id,
            side = %request.side,
            size = %request.size,
            price = %request.price,
            "venue tool entry"
        );
        let adapters = self.get_venue_adapters()?;

        // Check that Polymarket credentials have been derived — clone out
        // of the lock to avoid holding RwLockReadGuard across await.
        let creds = {
            let guard = adapters
                .polymarket_credentials
                .read()
                .expect("polymarket credentials read lock");
            match guard.get(&Self::polymarket_wallet_key(&request.wallet_address)) {
                Some(c) => c.clone(),
                None => {
                    tracing::error!(
                        venue = "polymarket",
                        wallet_address = %request.wallet_address,
                        "credentials not derived — call venue.derive_api_key first"
                    );
                    return Err(McpContractError::PolymarketCredentialsNotDerived);
                }
            }
        };

        let idempotency_key = format!(
            "pm-{}-{}",
            request.token_id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );

        let transport = Arc::new(a2ex_prediction_market_adapter::PolymarketHttpTransport::new(
            &adapters.polymarket_clob_base_url,
            adapters.signer.clone(),
            creds.clone(),
            &request.wallet_address,
        ));
        let prediction_market =
            a2ex_prediction_market_adapter::PredictionMarketAdapter::with_transport(transport);

        let (ack, status) = prediction_market
            .place_and_sync(a2ex_prediction_market_adapter::PredictionOrderRequest {
                venue: a2ex_prediction_market_adapter::PredictionVenue::Polymarket,
                market: request.token_id,
                side: request.side,
                size: request.size,
                price: request.price,
                max_fee_bps: 0,
                max_slippage_bps: 100,
                idempotency_key,
                auth: a2ex_prediction_market_adapter::PredictionAuth {
                    credential_id: creds.api_key.clone(),
                    auth_summary: "l2_hmac".to_owned(),
                },
            })
            .await
            .map_err(|e| {
                tracing::error!(venue = "polymarket", error = %e, "order placement failed");
                McpContractError::VenueTransport {
                    venue: "polymarket".to_owned(),
                    message: e.to_string(),
                }
            })?;

        Ok(TradePolymarketResponse {
            order_id: ack.order_id,
            status: status.status,
            filled_amount: if status.filled_amount != "0" {
                Some(status.filled_amount)
            } else {
                None
            },
            venue: "polymarket".to_owned(),
        })
    }

    /// Derive Polymarket API key via EIP-712 signing and store in server state.
    pub async fn handle_derive_api_key(
        &self,
        request: DeriveApiKeyRequest,
    ) -> Result<DeriveApiKeyResponse, McpContractError> {
        tracing::info!(
            tool = TOOL_VENUE_DERIVE_API_KEY,
            wallet_address = %request.wallet_address,
            "venue tool entry"
        );
        let adapters = self.get_venue_adapters()?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let nonce = "0".to_owned();

        let params = a2ex_prediction_market_adapter::signing::ClobAuthParams {
            address: request.wallet_address.clone(),
            timestamp: timestamp.clone(),
            nonce: nonce.clone(),
            message: "This message attests that I control the given wallet".to_owned(),
        };
        let sign_request =
            a2ex_prediction_market_adapter::signing::build_clob_auth_eip712_request(&params);

        let signed = adapters
            .signer
            .sign_typed_data(sign_request)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "ClobAuth EIP-712 signing failed");
                McpContractError::SignerBridgeError {
                    message: e.to_string(),
                }
            })?;

        let signature_str = signed.signature_hex.as_deref().unwrap_or_else(|| {
            // Fallback: hex-encode the raw bytes
            ""
        });
        let signature_hex_owned;
        let sig_ref = if signature_str.is_empty() {
            signature_hex_owned = format!("0x{}", hex::encode(&signed.bytes));
            &signature_hex_owned
        } else {
            signature_str
        };

        let headers = a2ex_prediction_market_adapter::signing::build_l1_auth_headers(
            &request.wallet_address,
            sig_ref,
            &timestamp,
            &nonce,
        );

        // Try POST /auth/api-key first (create), fall back to GET /auth/derive-api-key (re-derive)
        let base = &adapters.polymarket_clob_base_url;
        let client = reqwest::Client::new();

        // Step A: Create API key (POST)
        let create_url = format!("{base}/auth/api-key");
        let mut create_builder = client.post(&create_url);
        for (key, value) in &headers {
            create_builder = create_builder.header(key, value);
        }
        let create_response = create_builder.send().await;

        // If create succeeds, use it; otherwise fall back to derive
        let response = match create_response {
            Ok(r) if r.status().is_success() => r,
            _ => {
                // Step B: Derive existing key (GET)
                let derive_url = format!("{base}/auth/derive-api-key");
                let mut derive_builder = client.get(&derive_url);
                for (key, value) in &headers {
                    derive_builder = derive_builder.header(key, value);
                }
                derive_builder.send().await.map_err(|e| {
                    McpContractError::VenueTransport {
                        venue: "polymarket".to_owned(),
                        message: e.to_string(),
                    }
                })?
            }
        };

        // response is already obtained from create or derive above

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::error!(
                status = %status,
                body = %body,
                "derive-api-key returned non-success status"
            );
            return Err(McpContractError::VenueTransport {
                venue: "polymarket".to_owned(),
                message: format!("derive-api-key HTTP {status}: {body}"),
            });
        }

        let credentials: PolymarketApiCredentials = response.json().await.map_err(|e| {
            tracing::error!(error = %e, "failed to parse derive-api-key response");
            McpContractError::VenueTransport {
                venue: "polymarket".to_owned(),
                message: format!("failed to parse credentials response: {e}"),
            }
        })?;

        // Store credentials keyed by wallet address.
        {
            let mut creds = adapters
                .polymarket_credentials
                .write()
                .expect("polymarket credentials write lock");
            creds.insert(Self::polymarket_wallet_key(&request.wallet_address), credentials);
        }

        Ok(DeriveApiKeyResponse {
            success: true,
            message:
                "API key derived and stored successfully for this wallet. You can now use venue.trade_polymarket with the same wallet_address."
                    .to_owned(),
        })
    }

    /// Query open positions across configured venues.
    pub async fn handle_query_positions(
        &self,
        request: QueryPositionsRequest,
    ) -> Result<QueryPositionsResponse, McpContractError> {
        tracing::info!(
            tool = TOOL_VENUE_QUERY_POSITIONS,
            venue = ?request.venue,
            "venue tool entry"
        );
        let adapters = self.get_venue_adapters()?;
        let mut positions = Vec::new();
        let venue_filter = request.venue.as_deref();

        // Query Hyperliquid positions
        if venue_filter.is_none() || venue_filter == Some("hyperliquid") {
            match adapters
                .hyperliquid
                .sync_state(a2ex_hyperliquid_adapter::HyperliquidSyncRequest {
                    signer_address: String::new(),
                    account_address: String::new(),
                    order_id: None,
                    aggregate_fills: false,
                })
                .await
            {
                Ok(snapshot) => {
                    for pos in snapshot.positions {
                        positions.push(crate::venue_tools::PositionEntry {
                            venue: "hyperliquid".to_owned(),
                            asset: pos.instrument.clone(),
                            size: pos.size.clone(),
                            entry_price: pos.entry_price.clone(),
                            pnl: "0".to_owned(), // PnL not in position struct
                        });
                    }
                }
                Err(e) => {
                    tracing::error!(venue = "hyperliquid", error = %e, "position query failed");
                    // Continue — don't fail the whole query for one venue
                }
            }
        }

        // Query Polymarket positions (if credentials available)
        if venue_filter.is_none() || venue_filter == Some("polymarket") {
            let has_creds = adapters
                .polymarket_credentials
                .read()
                .expect("polymarket credentials read lock")
                .is_empty()
                == false;
            if !has_creds {
                tracing::info!(
                    venue = "polymarket",
                    "skipping position query — credentials not derived"
                );
            }
            // Polymarket position query would go through the prediction market
            // adapter; for now we only report Hyperliquid positions since the
            // prediction market transport doesn't expose a position endpoint.
        }

        Ok(QueryPositionsResponse { positions })
    }

    /// Check the status of an Across bridge transfer.
    pub async fn handle_bridge_status(
        &self,
        request: BridgeStatusRequest,
    ) -> Result<BridgeStatusResponse, McpContractError> {
        tracing::info!(
            tool = TOOL_VENUE_BRIDGE_STATUS,
            deposit_id = %request.deposit_id,
            "venue tool entry"
        );
        let adapters = self.get_venue_adapters()?;
        let status = adapters
            .across
            .sync_status(&request.deposit_id)
            .await
            .map_err(|e| {
                tracing::error!(venue = "across", error = %e, "bridge status check failed");
                McpContractError::VenueTransport {
                    venue: "across".to_owned(),
                    message: e.to_string(),
                }
            })?;

        Ok(BridgeStatusResponse {
            deposit_id: status.deposit_id,
            status: status.status,
            fill_tx_hash: status.fill_tx_hash,
            destination_tx_id: status.destination_tx_id,
        })
    }

}
