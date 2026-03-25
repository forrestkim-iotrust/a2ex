use crate::chain_tools::{ChainBalanceRequest, ChainBalanceResponse, ChainExecuteRequest};
use crate::defi_tools::{DefiApproveRequest, DefiBridgeRequest};
use crate::server::A2exSkillMcpServer;
use crate::venue_recipes::{HyperliquidTradeRequest, PolymarketTradeRequest, VenueTradeResponse};

impl A2exSkillMcpServer {
    pub async fn handle_polymarket_trade(
        &self,
        request: PolymarketTradeRequest,
    ) -> Result<VenueTradeResponse, String> {
        let mut steps = Vec::new();
        let adapters = self.get_venue_adapters().map_err(|e| e.to_string())?;
        let wallet_addr = self.resolve_hot_wallet_address()?;

        // 1. Check Polygon USDC.e balance (Polymarket uses bridged USDC.e, not native USDC)
        let poly_balance = self
            .handle_chain_balance(ChainBalanceRequest {
                chain_id: 137,
                address: wallet_addr.clone(),
                tokens: vec!["0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174".into()],
            })
            .await
            .unwrap_or_else(|_| ChainBalanceResponse {
                native: crate::chain_tools::TokenBalance {
                    address: "native".into(), symbol: "POL".into(),
                    balance: "0".into(), raw: "0".into(), decimals: 18,
                },
                tokens: vec![],
                chain: "polygon".into(),
            });

        let poly_usdc = poly_balance.tokens.first().map(|t| t.raw.parse::<u128>().unwrap_or(0)).unwrap_or(0);
        let size_f: f64 = request.size.parse().unwrap_or(0.0);
        let price_f: f64 = request.price.parse().unwrap_or(0.0);
        let needed_usdc = (size_f * price_f * 1_000_000.0) as u128;

        steps.push(format!("polygon USDC: {} raw, needed: {} raw", poly_usdc, needed_usdc));

        // 2. Bridge if needed
        if poly_usdc < needed_usdc {
            let bridge_amount = (needed_usdc - poly_usdc) as f64 / 1_000_000.0 + 0.01; // buffer
            steps.push(format!("bridging {} USDC Arb→Polygon", bridge_amount));

            let bridge_result = self
                .handle_defi_bridge(DefiBridgeRequest {
                    from_chain: 42161,
                    to_chain: 137,
                    token: "USDC".into(),
                    amount: format!("{:.2}", bridge_amount),
                    output_token: Some("0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174".into()), // USDC.e
                })
                .await?;

            if bridge_result.status != "filled" {
                return Ok(VenueTradeResponse {
                    venue: "polymarket".into(),
                    order_id: None,
                    status: "failed".into(),
                    cost: None,
                    error: Some(format!("bridge failed: {:?}", bridge_result.error)),
                    steps,
                });
            }
            steps.push(format!("bridge filled: {:?}", bridge_result.fill_tx));
        }

        // 3. Check POL gas
        let pol_raw: u128 = poly_balance.native.raw.parse().unwrap_or(0);
        if pol_raw < 1_000_000_000_000_000 {
            // < 0.001 POL — need gas
            steps.push("need POL gas, bridging USDC→native POL".into());
            let gas_bridge = self
                .handle_defi_bridge(DefiBridgeRequest {
                    from_chain: 42161,
                    to_chain: 137,
                    token: "USDC".into(),
                    amount: "0.10".into(),
                    output_token: Some("native".into()),
                })
                .await;
            match gas_bridge {
                Ok(r) if r.status == "filled" => steps.push("POL gas bridge filled".into()),
                _ => steps.push("POL gas bridge failed (may still work if gas exists)".into()),
            }
        }

        // 4. Approve CTF Exchange
        let exchange_addr = if request.neg_risk {
            "0xC5d563A36AE78145C45a50134d48A1215220f80a"
        } else {
            "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E"
        };

        steps.push(format!("approving CTF Exchange {}", &exchange_addr[..10]));
        let approve_result = self
            .handle_defi_approve(DefiApproveRequest {
                chain_id: 137,
                token: "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174".into(), // USDC.e
                spender: exchange_addr.into(),
                amount: None,
            })
            .await;

        match approve_result {
            Ok(r) if r.status == "confirmed" => steps.push("CTF approve confirmed".into()),
            Ok(r) => steps.push(format!("CTF approve: {} {:?}", r.status, r.error)),
            Err(e) => steps.push(format!("CTF approve error (may already be approved): {}", e)),
        }

        // 5. Derive credentials
        steps.push("deriving Polymarket credentials".into());
        let derive_result = self
            .handle_derive_api_key(crate::venue_tools::DeriveApiKeyRequest {
                wallet_address: wallet_addr.clone(),
            })
            .await;

        match derive_result {
            Ok(_) => steps.push("credentials derived".into()),
            Err(e) => {
                return Ok(VenueTradeResponse {
                    venue: "polymarket".into(),
                    order_id: None,
                    status: "failed".into(),
                    cost: None,
                    error: Some(format!("credential derivation failed: {e}")),
                    steps,
                });
            }
        }

        // 6. Place order
        steps.push(format!("placing order: {} {} @ {}", request.side, request.size, request.price));
        let trade_result = self
            .handle_trade_polymarket(crate::venue_tools::TradePolymarketRequest {
                token_id: request.token_id,
                wallet_address: wallet_addr,
                side: request.side,
                size: request.size.clone(),
                price: request.price.clone(),
                order_type: "limit".into(),
            })
            .await;

        match trade_result {
            Ok(r) => Ok(VenueTradeResponse {
                venue: "polymarket".into(),
                order_id: Some(r.order_id),
                status: r.status,
                cost: Some(format!("${:.2}", size_f * price_f)),
                error: None,
                steps,
            }),
            Err(e) => Ok(VenueTradeResponse {
                venue: "polymarket".into(),
                order_id: None,
                status: "failed".into(),
                cost: None,
                error: Some(e.to_string()),
                steps,
            }),
        }
    }

    pub async fn handle_hyperliquid_trade(
        &self,
        request: HyperliquidTradeRequest,
    ) -> Result<VenueTradeResponse, String> {
        let mut steps = Vec::new();

        // 1. Try placing order directly — if account exists, it works
        steps.push(format!("placing order: {} {} {} @ {}", if request.is_buy {"buy"} else {"sell"}, request.size, request.asset, request.price));

        let trade_result = self
            .handle_trade_hyperliquid(crate::venue_tools::TradeHyperliquidRequest {
                asset: request.asset.clone(),
                is_buy: request.is_buy,
                size: request.size.clone(),
                price: request.price.clone(),
                order_type: request.order_type.clone(),
                reduce_only: false,
            })
            .await;

        match trade_result {
            Ok(r) => {
                steps.push("order submitted".into());
                return Ok(VenueTradeResponse {
                    venue: "hyperliquid".into(),
                    order_id: Some(r.order_id),
                    status: r.status,
                    cost: None,
                    error: None,
                    steps,
                });
            }
            Err(e) => {
                let err_str = e.to_string();
                if !err_str.contains("does not exist") {
                    return Ok(VenueTradeResponse {
                        venue: "hyperliquid".into(),
                        order_id: None,
                        status: "failed".into(),
                        cost: None,
                        error: Some(err_str),
                        steps,
                    });
                }
                steps.push("account not found — need USDC deposit to Hyperliquid".into());
            }
        }

        // 2. Account doesn't exist — deposit USDC via Arbitrum bridge
        let wallet_addr = self.resolve_hot_wallet_address()?;
        let deposit_amount = request.size.parse::<f64>().unwrap_or(0.0)
            * request.price.parse::<f64>().unwrap_or(0.0);
        let deposit_usdc = (deposit_amount * 1.1 + 5.0).max(5.0); // +10% buffer, $5 min
        let deposit_raw = (deposit_usdc * 1_000_000.0) as u128;

        steps.push(format!("depositing {} USDC to Hyperliquid bridge", deposit_usdc));

        // 2a. Check Arb USDC balance
        let arb_balance = self
            .handle_chain_balance(ChainBalanceRequest {
                chain_id: 42161,
                address: wallet_addr.clone(),
                tokens: vec!["0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into()],
            })
            .await
            .unwrap_or_else(|_| ChainBalanceResponse {
                native: crate::chain_tools::TokenBalance {
                    address: "native".into(), symbol: "ETH".into(),
                    balance: "0".into(), raw: "0".into(), decimals: 18,
                },
                tokens: vec![],
                chain: "arbitrum".into(),
            });

        let arb_usdc = arb_balance.tokens.first()
            .map(|t| t.raw.parse::<u128>().unwrap_or(0)).unwrap_or(0);

        if arb_usdc < deposit_raw {
            return Ok(VenueTradeResponse {
                venue: "hyperliquid".into(),
                order_id: None,
                status: "needs_funding".into(),
                cost: None,
                error: Some(format!(
                    "need {} USDC on Arbitrum for Hyperliquid deposit, have {}",
                    deposit_usdc,
                    arb_usdc as f64 / 1_000_000.0
                )),
                steps,
            });
        }

        // 2b. Approve USDC for Hyperliquid bridge
        let hl_bridge = "0x2Df1c51E09aECF9cacB7bc98cB1742757f163dF7";
        steps.push("approving USDC for Hyperliquid bridge".into());
        let _ = self
            .handle_defi_approve(DefiApproveRequest {
                chain_id: 42161,
                token: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into(),
                spender: hl_bridge.into(),
                amount: None,
            })
            .await;

        // 2c. Transfer USDC to bridge contract — ERC20 transfer(address,uint256)
        let bridge_clean = hl_bridge.strip_prefix("0x").unwrap_or(hl_bridge).to_lowercase();
        let amount_hex = format!("{:064x}", deposit_raw);
        let transfer_data = format!("0xa9059cbb000000000000000000000000{bridge_clean}{amount_hex}");

        steps.push(format!("transferring {} USDC to bridge {}", deposit_usdc, &hl_bridge[..10]));
        let transfer_result = self
            .handle_chain_execute(ChainExecuteRequest {
                chain_id: 42161,
                to: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into(),
                calldata: transfer_data,
                value: Some("0".into()),
            })
            .await?;

        if transfer_result.status != "confirmed" {
            return Ok(VenueTradeResponse {
                venue: "hyperliquid".into(),
                order_id: None,
                status: "failed".into(),
                cost: None,
                error: Some(format!("bridge deposit failed: {:?}", transfer_result.error)),
                steps,
            });
        }

        steps.push(format!("deposit confirmed: {}", transfer_result.tx_hash));

        // 3. Wait for deposit credit, then retry order (2 attempts, 30s apart)
        for attempt in 1..=2 {
            steps.push(format!("waiting 30s for deposit credit (attempt {attempt}/2)"));
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;

            let retry_result = self
                .handle_trade_hyperliquid(crate::venue_tools::TradeHyperliquidRequest {
                    asset: request.asset.clone(),
                    is_buy: request.is_buy,
                    size: request.size.clone(),
                    price: request.price.clone(),
                    order_type: request.order_type.clone(),
                    reduce_only: false,
                })
                .await;

            match retry_result {
                Ok(r) => {
                    steps.push(format!("order submitted after deposit (attempt {attempt})"));
                    return Ok(VenueTradeResponse {
                        venue: "hyperliquid".into(),
                        order_id: Some(r.order_id),
                        status: r.status,
                        cost: None,
                        error: None,
                        steps,
                    });
                }
                Err(e) if attempt < 2 => {
                    steps.push(format!("retry {attempt} failed: {e} — will retry"));
                }
                Err(e) => {
                    return Ok(VenueTradeResponse {
                        venue: "hyperliquid".into(),
                        order_id: None,
                        status: "deposit_sent".into(),
                        cost: None,
                        error: Some(format!("deposit confirmed but order failed after 2 attempts: {e}")),
                        steps,
                    });
                }
            }
        }
        unreachable!()
    }
}
