//! Layer 1: Chain Primitives — low-level blockchain interaction tools.
//!
//! These tools give the AI direct access to read, simulate, execute, and
//! query balances on any EVM chain. They form the foundation for Layer 2
//! (DeFi primitives) and Layer 3 (venue recipes).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool name constants
// ---------------------------------------------------------------------------

pub const TOOL_CHAIN_READ: &str = "chain_read";
pub const TOOL_CHAIN_EXECUTE: &str = "chain_execute";
pub const TOOL_CHAIN_BALANCE: &str = "chain_balance";
pub const TOOL_CHAIN_SIMULATE: &str = "chain_simulate";

// ---------------------------------------------------------------------------
// Chain registry — chain_id → RPC URL
// ---------------------------------------------------------------------------

pub fn rpc_url_for_chain(chain_id: u64) -> Option<&'static str> {
    match chain_id {
        1 => Some("https://eth.drpc.org"),
        10 => Some("https://optimism.drpc.org"),
        137 => Some("https://polygon-bor-rpc.publicnode.com"),
        8453 => Some("https://base.drpc.org"),
        42161 => Some("https://arb1.arbitrum.io/rpc"),
        _ => None,
    }
}

pub fn chain_name(chain_id: u64) -> &'static str {
    match chain_id {
        1 => "ethereum",
        10 => "optimism",
        137 => "polygon",
        8453 => "base",
        42161 => "arbitrum",
        _ => "unknown",
    }
}

/// Map chain_id to WAIaaS network identifier.
pub fn waiaas_network(chain_id: u64) -> Option<&'static str> {
    match chain_id {
        1 => Some("ethereum-mainnet"),
        10 => Some("optimism-mainnet"),
        137 => Some("polygon-mainnet"),
        8453 => Some("base-mainnet"),
        42161 => Some("arbitrum-mainnet"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Read on-chain data via eth_call (view function).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainReadRequest {
    /// EVM chain ID (1=Ethereum, 137=Polygon, 42161=Arbitrum, etc.)
    pub chain_id: u64,
    /// Contract address to call.
    pub to: String,
    /// ABI-encoded calldata (hex with 0x prefix).
    pub calldata: String,
}

/// Execute an on-chain transaction (state-changing).
/// Automatically simulates before executing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainExecuteRequest {
    /// EVM chain ID.
    pub chain_id: u64,
    /// Contract address to call.
    pub to: String,
    /// ABI-encoded calldata (hex with 0x prefix).
    pub calldata: String,
    /// ETH/native value to send in wei (decimal string). Defaults to "0".
    #[serde(default)]
    pub value: Option<String>,
}

/// Query native + ERC-20 token balances.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainBalanceRequest {
    /// EVM chain ID.
    pub chain_id: u64,
    /// Wallet address to query.
    pub address: String,
    /// Optional list of ERC-20 token addresses to check. If empty, checks common tokens.
    #[serde(default)]
    pub tokens: Vec<String>,
}

/// Simulate a transaction without executing (dry-run).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainSimulateRequest {
    /// EVM chain ID.
    pub chain_id: u64,
    /// Contract address to call.
    pub to: String,
    /// ABI-encoded calldata (hex with 0x prefix).
    pub calldata: String,
    /// ETH/native value in wei (decimal string). Defaults to "0".
    #[serde(default)]
    pub value: Option<String>,
    /// Address to simulate from. Defaults to the hot wallet.
    #[serde(default)]
    pub from: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainReadResponse {
    /// Hex-encoded return data from the call.
    pub result: String,
    /// Chain name for context.
    pub chain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainExecuteResponse {
    /// Transaction hash.
    pub tx_hash: String,
    /// Transaction status: "confirmed" or "failed".
    pub status: String,
    /// Gas used.
    #[serde(default)]
    pub gas_used: Option<u64>,
    /// Error message if failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Chain name.
    pub chain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TokenBalance {
    /// Token contract address (or "native" for ETH/POL/etc).
    pub address: String,
    /// Token symbol if known.
    pub symbol: String,
    /// Human-readable balance (e.g. "10.5").
    pub balance: String,
    /// Raw balance (e.g. "10500000").
    pub raw: String,
    /// Token decimals.
    pub decimals: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainBalanceResponse {
    /// Native token balance.
    pub native: TokenBalance,
    /// ERC-20 token balances.
    pub tokens: Vec<TokenBalance>,
    /// Chain name.
    pub chain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChainSimulateResponse {
    /// Whether the transaction would succeed.
    pub success: bool,
    /// Estimated gas if successful.
    #[serde(default)]
    pub gas_estimate: Option<u64>,
    /// Revert reason if failed.
    #[serde(default)]
    pub revert_reason: Option<String>,
    /// Chain name.
    pub chain: String,
}

// ---------------------------------------------------------------------------
// Common tokens per chain (for auto-scan in chain.balance)
// ---------------------------------------------------------------------------

pub fn common_tokens(chain_id: u64) -> Vec<(&'static str, &'static str, u8)> {
    // (address, symbol, decimals)
    match chain_id {
        1 => vec![
            ("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", "USDC", 6),
            ("0xdAC17F958D2ee523a2206206994597C13D831ec7", "USDT", 6),
            ("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", "WETH", 18),
        ],
        42161 => vec![
            ("0xaf88d065e77c8cC2239327C5EDb3A432268e5831", "USDC", 6),
            ("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9", "USDT", 6),
            ("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", "WETH", 18),
        ],
        137 => vec![
            ("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", "USDC", 6),
            ("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270", "WMATIC", 18),
        ],
        8453 => vec![
            ("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "USDC", 6),
            ("0x4200000000000000000000000000000000000006", "WETH", 18),
        ],
        10 => vec![
            ("0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85", "USDC", 6),
            ("0x4200000000000000000000000000000000000006", "WETH", 18),
        ],
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// RPC helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- rpc_url_for_chain --

    #[test]
    fn rpc_url_known_chains() {
        assert!(rpc_url_for_chain(1).unwrap().contains("eth"));
        assert!(rpc_url_for_chain(10).unwrap().contains("optimism"));
        assert!(rpc_url_for_chain(137).unwrap().contains("polygon"));
        assert!(rpc_url_for_chain(8453).unwrap().contains("base"));
        assert!(rpc_url_for_chain(42161).unwrap().contains("arb"));
    }

    #[test]
    fn rpc_url_unknown_chain_returns_none() {
        assert!(rpc_url_for_chain(0).is_none());
        assert!(rpc_url_for_chain(999999).is_none());
    }

    // -- chain_name --

    #[test]
    fn chain_name_known() {
        assert_eq!(chain_name(1), "ethereum");
        assert_eq!(chain_name(10), "optimism");
        assert_eq!(chain_name(137), "polygon");
        assert_eq!(chain_name(8453), "base");
        assert_eq!(chain_name(42161), "arbitrum");
    }

    #[test]
    fn chain_name_unknown_returns_unknown() {
        assert_eq!(chain_name(0), "unknown");
        assert_eq!(chain_name(56), "unknown");
    }

    // -- waiaas_network --

    #[test]
    fn waiaas_network_known_chains() {
        assert_eq!(waiaas_network(1), Some("ethereum-mainnet"));
        assert_eq!(waiaas_network(10), Some("optimism-mainnet"));
        assert_eq!(waiaas_network(137), Some("polygon-mainnet"));
        assert_eq!(waiaas_network(8453), Some("base-mainnet"));
        assert_eq!(waiaas_network(42161), Some("arbitrum-mainnet"));
    }

    #[test]
    fn waiaas_network_unknown_returns_none() {
        assert!(waiaas_network(56).is_none());
    }

    // -- common_tokens --

    #[test]
    fn common_tokens_arbitrum_has_usdc() {
        let tokens = common_tokens(42161);
        assert!(tokens.iter().any(|(_, sym, _)| *sym == "USDC"));
        let usdc = tokens.iter().find(|(_, sym, _)| *sym == "USDC").unwrap();
        assert_eq!(usdc.2, 6); // decimals
    }

    #[test]
    fn common_tokens_polygon_has_usdc() {
        let tokens = common_tokens(137);
        assert!(tokens.iter().any(|(_, sym, _)| *sym == "USDC"));
    }

    #[test]
    fn common_tokens_unknown_chain_empty() {
        assert!(common_tokens(999).is_empty());
    }

    #[test]
    fn common_tokens_addresses_are_checksummed() {
        for chain_id in [1, 42161, 137, 8453, 10] {
            for (addr, _, _) in common_tokens(chain_id) {
                assert!(
                    addr.starts_with("0x"),
                    "token address on chain {chain_id} missing 0x prefix: {addr}"
                );
                assert_eq!(
                    addr.len(),
                    42,
                    "token address on chain {chain_id} wrong length: {addr}"
                );
            }
        }
    }

    // -- request/response serialization --

    #[test]
    fn chain_read_request_roundtrips_json() {
        let req = ChainReadRequest {
            chain_id: 42161,
            to: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into(),
            calldata: "0x70a08231".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ChainReadRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.chain_id, 42161);
        assert_eq!(back.to, req.to);
    }

    #[test]
    fn chain_execute_request_value_defaults_none() {
        let json = r#"{"chain_id":1,"to":"0x00","calldata":"0x00"}"#;
        let req: ChainExecuteRequest = serde_json::from_str(json).unwrap();
        assert!(req.value.is_none());
    }

    #[test]
    fn chain_balance_request_tokens_defaults_empty() {
        let json = r#"{"chain_id":1,"address":"0x00"}"#;
        let req: ChainBalanceRequest = serde_json::from_str(json).unwrap();
        assert!(req.tokens.is_empty());
    }

    #[test]
    fn chain_simulate_request_from_defaults_none() {
        let json = r#"{"chain_id":1,"to":"0x00","calldata":"0x00"}"#;
        let req: ChainSimulateRequest = serde_json::from_str(json).unwrap();
        assert!(req.from.is_none());
        assert!(req.value.is_none());
    }

    #[test]
    fn json_schema_generation_succeeds() {
        // Verify #[derive(JsonSchema)] doesn't panic
        let _ = schemars::schema_for!(ChainReadRequest);
        let _ = schemars::schema_for!(ChainExecuteRequest);
        let _ = schemars::schema_for!(ChainBalanceRequest);
        let _ = schemars::schema_for!(ChainSimulateRequest);
        let _ = schemars::schema_for!(ChainReadResponse);
        let _ = schemars::schema_for!(ChainExecuteResponse);
        let _ = schemars::schema_for!(ChainBalanceResponse);
        let _ = schemars::schema_for!(ChainSimulateResponse);
    }
}

/// Make a JSON-RPC call to the chain.
pub async fn rpc_call(
    client: &reqwest::Client,
    rpc_url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("RPC request failed: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("RPC response parse failed: {e}"))?;

    if let Some(error) = json.get("error") {
        let msg = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown RPC error");
        return Err(format!("RPC error: {msg}"));
    }

    json.get("result")
        .cloned()
        .ok_or_else(|| "RPC response missing result".to_string())
}
