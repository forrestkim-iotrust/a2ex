use crate::chain_tools::{
    self, ChainBalanceRequest, ChainBalanceResponse, ChainExecuteRequest, ChainExecuteResponse,
    ChainReadRequest, ChainReadResponse, ChainSimulateRequest, ChainSimulateResponse, TokenBalance,
};
use crate::server::A2exSkillMcpServer;

impl A2exSkillMcpServer {
    pub async fn handle_chain_read(
        &self,
        request: ChainReadRequest,
    ) -> Result<ChainReadResponse, String> {
        let rpc_url = chain_tools::rpc_url_for_chain(request.chain_id)
            .ok_or_else(|| format!("unsupported chain_id: {}", request.chain_id))?;

        let result = chain_tools::rpc_call(
            &self.client,
            rpc_url,
            "eth_call",
            serde_json::json!([{
                "to": request.to,
                "data": request.calldata,
            }, "latest"]),
        )
        .await?;

        Ok(ChainReadResponse {
            result: result.as_str().unwrap_or("0x").to_string(),
            chain: chain_tools::chain_name(request.chain_id).to_string(),
        })
    }

    pub async fn handle_chain_simulate(
        &self,
        request: ChainSimulateRequest,
    ) -> Result<ChainSimulateResponse, String> {
        let rpc_url = chain_tools::rpc_url_for_chain(request.chain_id)
            .ok_or_else(|| format!("unsupported chain_id: {}", request.chain_id))?;
        let chain = chain_tools::chain_name(request.chain_id).to_string();
        let value = request.value.as_deref().unwrap_or("0");
        let value_hex = format!("0x{:x}", value.parse::<u64>().unwrap_or(0));

        let from = request
            .from
            .as_deref()
            .unwrap_or("0x0000000000000000000000000000000000000000");

        // eth_call to check revert
        let call_result = chain_tools::rpc_call(
            &self.client,
            rpc_url,
            "eth_call",
            serde_json::json!([{
                "from": from,
                "to": request.to,
                "data": request.calldata,
                "value": value_hex,
            }, "latest"]),
        )
        .await;

        match call_result {
            Err(e) => Ok(ChainSimulateResponse {
                success: false,
                gas_estimate: None,
                revert_reason: Some(e),
                chain,
            }),
            Ok(_) => {
                // eth_estimateGas
                let gas = chain_tools::rpc_call(
                    &self.client,
                    rpc_url,
                    "eth_estimateGas",
                    serde_json::json!([{
                        "from": from,
                        "to": request.to,
                        "data": request.calldata,
                        "value": value_hex,
                    }]),
                )
                .await;

                let gas_estimate = gas.ok().and_then(|v| {
                    v.as_str().map(|s| {
                        u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
                    })
                });

                Ok(ChainSimulateResponse {
                    success: true,
                    gas_estimate,
                    revert_reason: None,
                    chain,
                })
            }
        }
    }

    pub async fn handle_chain_balance(
        &self,
        request: ChainBalanceRequest,
    ) -> Result<ChainBalanceResponse, String> {
        let rpc_url = chain_tools::rpc_url_for_chain(request.chain_id)
            .ok_or_else(|| format!("unsupported chain_id: {}", request.chain_id))?;
        let chain = chain_tools::chain_name(request.chain_id).to_string();
        let addr = &request.address;

        // Native balance
        let native_result = chain_tools::rpc_call(
            &self.client,
            rpc_url,
            "eth_getBalance",
            serde_json::json!([addr, "latest"]),
        )
        .await?;

        let native_raw = u128::from_str_radix(
            native_result
                .as_str()
                .unwrap_or("0x0")
                .trim_start_matches("0x"),
            16,
        )
        .unwrap_or(0);

        let native_symbol = match request.chain_id {
            137 => "POL",
            _ => "ETH",
        };

        let native = TokenBalance {
            address: "native".to_string(),
            symbol: native_symbol.to_string(),
            balance: format!("{:.6}", native_raw as f64 / 1e18),
            raw: native_raw.to_string(),
            decimals: 18,
        };

        // ERC-20 tokens
        let token_list: Vec<(String, String, u8)> = if request.tokens.is_empty() {
            chain_tools::common_tokens(request.chain_id)
                .iter()
                .map(|(a, s, d)| (a.to_string(), s.to_string(), *d))
                .collect()
        } else {
            request
                .tokens
                .iter()
                .map(|a| (a.clone(), "???".to_string(), 18))
                .collect()
        };

        let mut tokens = Vec::new();
        // balanceOf(address) = 0x70a08231
        for (token_addr, symbol, decimals) in &token_list {
            let addr_clean = addr
                .strip_prefix("0x")
                .unwrap_or(addr)
                .to_lowercase();
            let calldata = format!("0x70a08231000000000000000000000000{}", addr_clean);

            if let Ok(result) = chain_tools::rpc_call(
                &self.client,
                rpc_url,
                "eth_call",
                serde_json::json!([{ "to": token_addr, "data": calldata }, "latest"]),
            )
            .await
            {
                let raw = u128::from_str_radix(
                    result
                        .as_str()
                        .unwrap_or("0x0")
                        .trim_start_matches("0x"),
                    16,
                )
                .unwrap_or(0);

                if raw > 0 {
                    let divisor = 10u128.pow(*decimals as u32);
                    tokens.push(TokenBalance {
                        address: token_addr.clone(),
                        symbol: symbol.clone(),
                        balance: format!("{:.6}", raw as f64 / divisor as f64),
                        raw: raw.to_string(),
                        decimals: *decimals,
                    });
                }
            }
        }

        Ok(ChainBalanceResponse {
            native,
            tokens,
            chain,
        })
    }

    pub async fn handle_chain_execute(
        &self,
        request: ChainExecuteRequest,
    ) -> Result<ChainExecuteResponse, String> {
        let chain = chain_tools::chain_name(request.chain_id).to_string();
        let network = chain_tools::waiaas_network(request.chain_id)
            .ok_or_else(|| format!("unsupported chain_id for execute: {}", request.chain_id))?;

        let _adapters = self.get_venue_adapters().map_err(|e| e.to_string())?;
        let value = request.value.as_deref().unwrap_or("0").to_string();

        // Auto-simulate first — use hot wallet address as sender
        let sim_from = self.resolve_hot_wallet_address().ok();
        let sim = self
            .handle_chain_simulate(ChainSimulateRequest {
                chain_id: request.chain_id,
                to: request.to.clone(),
                calldata: request.calldata.clone(),
                value: Some(value.clone()),
                from: sim_from,
            })
            .await?;

        if !sim.success {
            return Ok(ChainExecuteResponse {
                tx_hash: String::new(),
                status: "simulation_failed".to_string(),
                gas_used: None,
                error: sim.revert_reason,
                chain,
            });
        }

        // Submit via WAIaaS signer bridge (sign + send)
        let waiaas_base = std::env::var("A2EX_WAIAAS_BASE_URL").unwrap_or_default();
        let session_token = std::env::var("A2EX_HOT_SESSION_TOKEN").unwrap_or_default();
        let wallet_id = std::env::var("A2EX_HOT_WALLET_ID").unwrap_or_default();

        let send_body = serde_json::json!({
            "walletId": wallet_id,
            "network": network,
            "type": "CONTRACT_CALL",
            "to": request.to,
            "calldata": request.calldata,
            "value": value,
        });

        let resp = self
            .client
            .post(format!("{waiaas_base}/v1/transactions/send"))
            .header("Authorization", format!("Bearer {session_token}"))
            .json(&send_body)
            .send()
            .await
            .map_err(|e| format!("WAIaaS send failed: {e}"))?;

        let resp_json: serde_json::Value =
            resp.json().await.map_err(|e| format!("WAIaaS parse: {e}"))?;

        let tx_id = resp_json
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if tx_id.is_empty() {
            let err = resp_json.to_string();
            return Ok(ChainExecuteResponse {
                tx_hash: String::new(),
                status: "submit_failed".to_string(),
                gas_used: None,
                error: Some(err),
                chain,
            });
        }

        // Poll for confirmation (up to 30s)
        for _ in 0..15 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let check = self
                .client
                .get(format!("{waiaas_base}/v1/transactions/{tx_id}"))
                .header("Authorization", format!("Bearer {session_token}"))
                .send()
                .await;

            if let Ok(r) = check {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    let status = j.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    if status == "CONFIRMED" {
                        let hash = j
                            .get("txHash")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        return Ok(ChainExecuteResponse {
                            tx_hash: hash,
                            status: "confirmed".to_string(),
                            gas_used: None,
                            error: None,
                            chain,
                        });
                    }
                    if status == "FAILED" {
                        let err = j
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        return Ok(ChainExecuteResponse {
                            tx_hash: String::new(),
                            status: "failed".to_string(),
                            gas_used: None,
                            error: Some(err),
                            chain,
                        });
                    }
                }
            }
        }

        Ok(ChainExecuteResponse {
            tx_hash: String::new(),
            status: "pending".to_string(),
            gas_used: None,
            error: Some(format!("tx {tx_id} still pending after 30s")),
            chain,
        })
    }
}
