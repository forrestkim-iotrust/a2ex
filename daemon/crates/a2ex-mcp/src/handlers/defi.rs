use crate::chain_tools::{self, ChainExecuteRequest};
use crate::defi_tools::{
    self, DefiAnalyzeRequest, DefiAnalyzeResponse, DefiApproveRequest, DefiApproveResponse,
    DefiBridgeRequest, DefiBridgeResponse,
};
use crate::server::A2exSkillMcpServer;

impl A2exSkillMcpServer {
    pub async fn handle_defi_approve(
        &self,
        request: DefiApproveRequest,
    ) -> Result<DefiApproveResponse, String> {
        let (token_addr, decimals) = if request.token.starts_with("0x") {
            (request.token.clone(), 18u8)
        } else {
            defi_tools::resolve_token(request.chain_id, &request.token)
                .ok_or_else(|| format!("unknown token: {}", request.token))?
        };

        let amount_raw = match &request.amount {
            Some(amt) => defi_tools::to_raw(amt, decimals).to_string(),
            None => "MAX".to_string(),
        };

        let calldata = defi_tools::build_approve_calldata(&request.spender, &amount_raw);

        let result = self
            .handle_chain_execute(ChainExecuteRequest {
                chain_id: request.chain_id,
                to: token_addr,
                calldata,
                value: None,
            })
            .await?;

        let approved = if request.amount.is_none() {
            "unlimited".to_string()
        } else {
            request.amount.unwrap_or_default()
        };

        Ok(DefiApproveResponse {
            tx_hash: result.tx_hash,
            status: result.status,
            amount: approved,
            error: result.error,
        })
    }

    pub async fn handle_defi_bridge(
        &self,
        request: DefiBridgeRequest,
    ) -> Result<DefiBridgeResponse, String> {
        let mut steps = Vec::new();

        // 1. Resolve input token
        let (input_token, input_decimals) =
            defi_tools::resolve_token(request.from_chain, &request.token)
                .ok_or_else(|| format!("unknown token {} on chain {}", request.token, request.from_chain))?;

        // 2. Resolve output token
        let output_token_addr = match &request.output_token {
            Some(t) => {
                let (addr, _) = defi_tools::resolve_token(request.to_chain, t)
                    .ok_or_else(|| format!("unknown output token {} on chain {}", t, request.to_chain))?;
                addr
            }
            None => {
                // Same token on destination
                let (addr, _) = defi_tools::resolve_token(request.to_chain, &request.token)
                    .ok_or_else(|| format!("no {} on chain {}", request.token, request.to_chain))?;
                addr
            }
        };

        let amount_raw = defi_tools::to_raw(&request.amount, input_decimals);
        steps.push(format!("resolved: {} {} = {} raw on chain {}", request.amount, request.token, amount_raw, request.from_chain));

        // 3. Get bridge quote via Across
        let adapters = self.get_venue_adapters().map_err(|e| e.to_string())?;
        let wallet_addr = self.resolve_hot_wallet_address()?;

        let quote = adapters
            .across
            .quote_bridge(a2ex_across_adapter::AcrossBridgeQuoteRequest {
                asset: input_token.clone(),
                amount_usd: amount_raw as u64,
                source_chain: request.from_chain.to_string(),
                destination_chain: request.to_chain.to_string(),
                depositor: if wallet_addr.is_empty() { None } else { Some(wallet_addr.clone()) },
                recipient: if wallet_addr.is_empty() { None } else { Some(wallet_addr.clone()) },
                output_token: if output_token_addr == "native" { None } else { Some(output_token_addr.clone()) },
            })
            .await
            .map_err(|e| format!("bridge quote failed: {e}"))?;

        steps.push(format!("bridge quote: input={:?} output={:?}", quote.input_amount, quote.output_amount));

        // 4. Check approval
        let spoke_pool = "0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A";
        if !quote.approval_txns.is_empty() {
            let atx = &quote.approval_txns[0];
            let approve_result = self
                .handle_chain_execute(ChainExecuteRequest {
                    chain_id: request.from_chain,
                    to: atx.to.clone(),
                    calldata: atx.data.clone(),
                    value: None,
                })
                .await?;

            if approve_result.status != "confirmed" {
                return Ok(DefiBridgeResponse {
                    status: "failed".into(),
                    deposit_tx: None,
                    fill_tx: None,
                    received: None,
                    fee: None,
                    error: Some(format!("approval failed: {:?}", approve_result.error)),
                    steps,
                });
            }
            steps.push(format!("approval confirmed: {}", approve_result.tx_hash));
        } else {
            steps.push("approval not needed".into());
        }

        // 5. Submit bridge deposit
        let swap_data = match &quote.swap_tx {
            Some(tx) => tx.data.clone(),
            None => quote.calldata.clone().unwrap_or_default(),
        };
        let swap_to = match &quote.swap_tx {
            Some(tx) => tx.to.clone(),
            None => spoke_pool.to_string(),
        };

        let deposit_result = self
            .handle_chain_execute(ChainExecuteRequest {
                chain_id: request.from_chain,
                to: swap_to,
                calldata: swap_data,
                value: Some("0".into()),
            })
            .await?;

        if deposit_result.status != "confirmed" {
            return Ok(DefiBridgeResponse {
                status: "failed".into(),
                deposit_tx: None,
                fill_tx: None,
                received: None,
                fee: None,
                error: Some(format!("bridge deposit failed: {:?}", deposit_result.error)),
                steps,
            });
        }

        let deposit_tx = deposit_result.tx_hash.clone();
        steps.push(format!("deposit confirmed: {}", deposit_tx));

        // 6. Poll bridge status
        for i in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let status = adapters
                .across
                .sync_status(&deposit_tx)
                .await;

            match status {
                Ok(s) if s.status == "filled" => {
                    steps.push(format!("bridge filled: {:?}", s.fill_tx_hash));
                    return Ok(DefiBridgeResponse {
                        status: "filled".into(),
                        deposit_tx: Some(deposit_tx),
                        fill_tx: s.fill_tx_hash,
                        received: quote.output_amount.clone(),
                        fee: Some(format!("{}", quote.bridge_fee_usd)),
                        error: None,
                        steps,
                    });
                }
                Ok(_) => {
                    if i % 5 == 0 {
                        steps.push(format!("polling... ({i}s)"));
                    }
                }
                Err(_) => {}
            }
        }

        steps.push("bridge fill timeout (60s)".into());
        Ok(DefiBridgeResponse {
            status: "pending".into(),
            deposit_tx: Some(deposit_tx),
            fill_tx: None,
            received: None,
            fee: None,
            error: Some("bridge fill not confirmed within 60s".into()),
            steps,
        })
    }

    pub async fn handle_defi_analyze(
        &self,
        request: DefiAnalyzeRequest,
    ) -> Result<DefiAnalyzeResponse, String> {
        let rpc_url = chain_tools::rpc_url_for_chain(request.chain_id)
            .ok_or_else(|| format!("unsupported chain_id: {}", request.chain_id))?;
        let chain = chain_tools::chain_name(request.chain_id).to_string();
        let addr_lower = request.address.to_lowercase();

        // Check known contracts first
        if let Some((name, risk)) = defi_tools::known_contract(request.chain_id, &addr_lower) {
            return Ok(DefiAnalyzeResponse {
                address: request.address,
                verified: true,
                name: Some(name.to_string()),
                functions: vec![], // known contracts don't need function listing
                risk_level: risk.to_string(),
                warnings: vec![],
                chain,
            });
        }

        // Check if it's a contract (has code)
        let code = chain_tools::rpc_call(
            &self.client,
            rpc_url,
            "eth_getCode",
            serde_json::json!([&request.address, "latest"]),
        )
        .await?;

        let code_str = code.as_str().unwrap_or("0x");
        if code_str == "0x" || code_str.is_empty() {
            return Ok(DefiAnalyzeResponse {
                address: request.address,
                verified: false,
                name: None,
                functions: vec![],
                risk_level: "critical".to_string(),
                warnings: vec!["Address has no contract code (EOA or empty)".into()],
                chain,
            });
        }

        // Try Etherscan/Blockscout API for verification
        let explorer_api = match request.chain_id {
            1 => Some("https://api.etherscan.io/api"),
            42161 => Some("https://api.arbiscan.io/api"),
            137 => Some("https://api.polygonscan.com/api"),
            8453 => Some("https://api.basescan.org/api"),
            _ => None,
        };

        let mut verified = false;
        let mut name = None;
        let mut functions = Vec::new();

        if let Some(api_url) = explorer_api {
            let url = format!(
                "{}?module=contract&action=getabi&address={}",
                api_url, request.address
            );
            if let Ok(resp) = self.client.get(&url).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if json.get("status").and_then(|s| s.as_str()) == Some("1") {
                        verified = true;
                        if let Some(abi_str) = json.get("result").and_then(|r| r.as_str()) {
                            if let Ok(abi) = serde_json::from_str::<Vec<serde_json::Value>>(abi_str) {
                                functions = abi
                                    .iter()
                                    .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("function"))
                                    .filter_map(|item| {
                                        let fn_name = item.get("name")?.as_str()?;
                                        let inputs: Vec<String> = item
                                            .get("inputs")?
                                            .as_array()?
                                            .iter()
                                            .filter_map(|i| i.get("type").and_then(|t| t.as_str()).map(|s| s.to_string()))
                                            .collect();
                                        Some(format!("{}({})", fn_name, inputs.join(",")))
                                    })
                                    .collect();
                            }
                        }
                    }
                }
            }

            // Get contract name
            let url2 = format!(
                "{}?module=contract&action=getsourcecode&address={}",
                api_url, request.address
            );
            if let Ok(resp) = self.client.get(&url2).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    name = json
                        .get("result")
                        .and_then(|r| r.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|item| item.get("ContractName"))
                        .and_then(|n| n.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                }
            }
        }

        let mut warnings = Vec::new();
        let risk_level = if verified {
            "medium".to_string() // verified but unknown — proceed with caution
        } else {
            warnings.push("Contract source code is NOT verified. Proceed with extreme caution.".into());
            "high".to_string()
        };

        if functions.is_empty() && verified {
            warnings.push("Could not parse ABI. Functions unknown.".into());
        }

        Ok(DefiAnalyzeResponse {
            address: request.address,
            verified,
            name,
            functions,
            risk_level,
            warnings,
            chain,
        })
    }
}
