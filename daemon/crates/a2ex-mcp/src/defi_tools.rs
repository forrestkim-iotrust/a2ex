//! Layer 2: DeFi Primitives — cross-chain bridge, token approval, swap.
//!
//! These tools automate multi-step DeFi operations that would otherwise
//! require the AI to orchestrate low-level chain.* calls manually.
//! Internally they compose Layer 1 chain primitives.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool name constants
// ---------------------------------------------------------------------------

pub const TOOL_DEFI_BRIDGE: &str = "defi_bridge";
pub const TOOL_DEFI_APPROVE: &str = "defi_approve";
pub const TOOL_DEFI_ANALYZE: &str = "defi_analyze";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Bridge tokens cross-chain. Handles gas, approval, deposit, and fill polling automatically.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefiBridgeRequest {
    /// Source chain ID (e.g. 42161 for Arbitrum).
    pub from_chain: u64,
    /// Destination chain ID (e.g. 137 for Polygon).
    pub to_chain: u64,
    /// Token symbol or address on source chain (e.g. "USDC" or "0xaf88...").
    pub token: String,
    /// Amount in human-readable units (e.g. "5.0" for 5 USDC).
    pub amount: String,
    /// Output token on destination. Defaults to same token. Use "native" for POL/ETH.
    #[serde(default)]
    pub output_token: Option<String>,
}

/// Approve an ERC-20 token for spending by a contract.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefiApproveRequest {
    /// Chain ID where the approval happens.
    pub chain_id: u64,
    /// ERC-20 token address to approve.
    pub token: String,
    /// Spender contract address (e.g. CTF Exchange, SpokePool).
    pub spender: String,
    /// Amount to approve in human units. Defaults to unlimited.
    #[serde(default)]
    pub amount: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefiBridgeResponse {
    /// Overall status: "filled", "pending", "failed".
    pub status: String,
    /// Deposit transaction hash on source chain.
    #[serde(default)]
    pub deposit_tx: Option<String>,
    /// Fill transaction hash on destination chain.
    #[serde(default)]
    pub fill_tx: Option<String>,
    /// Amount received on destination (human-readable).
    #[serde(default)]
    pub received: Option<String>,
    /// Bridge fee in USD.
    #[serde(default)]
    pub fee: Option<String>,
    /// Error message if failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Steps executed (for transparency).
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefiApproveResponse {
    /// Transaction hash of the approval.
    pub tx_hash: String,
    /// Status: "confirmed" or "failed".
    pub status: String,
    /// Amount approved (human-readable or "unlimited").
    pub amount: String,
    /// Error if failed.
    #[serde(default)]
    pub error: Option<String>,
}

/// Analyze a smart contract — fetch ABI, check verification, assess risk.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefiAnalyzeRequest {
    /// EVM chain ID.
    pub chain_id: u64,
    /// Contract address to analyze.
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefiAnalyzeResponse {
    /// Contract address.
    pub address: String,
    /// Whether the contract source code is verified.
    pub verified: bool,
    /// Contract name if verified.
    #[serde(default)]
    pub name: Option<String>,
    /// List of public functions (e.g. "transfer(address,uint256)").
    pub functions: Vec<String>,
    /// Risk level: "low", "medium", "high", "critical".
    pub risk_level: String,
    /// Risk warnings.
    pub warnings: Vec<String>,
    /// Chain name.
    pub chain: String,
}

// ---------------------------------------------------------------------------
// Well-known contract registry (for risk assessment)
// ---------------------------------------------------------------------------

/// Check if a contract is a well-known, trusted protocol.
pub fn known_contract(chain_id: u64, address: &str) -> Option<(&'static str, &'static str)> {
    let addr = address.to_lowercase();
    match (chain_id, addr.as_str()) {
        // Arbitrum
        (42161, "0xaf88d065e77c8cc2239327c5edb3a432268e5831") => Some(("USDC", "low")),
        (42161, "0xe35e9842fceaca96570b734083f4a58e8f7c5f2a") => Some(("Across SpokePool", "low")),
        (42161, "0x82af49447d8a07e3bd95bd0d56f35241523fbab1") => Some(("WETH", "low")),
        // Polygon
        (137, "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359") => Some(("USDC", "low")),
        (137, "0x4bfb41d5b3570defd03c39a9a4d8de6bd8b8982e") => Some(("Polymarket CTF Exchange", "low")),
        (137, "0xc5d563a36ae78145c45a50134d48a1215220f80a") => Some(("Polymarket NegRisk CTF Exchange", "low")),
        (137, "0x0d500b1d8e8ef31e21c99d1db9a6444d3adf1270") => Some(("WMATIC", "low")),
        // Ethereum
        (1, "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48") => Some(("USDC", "low")),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Token resolution helpers
// ---------------------------------------------------------------------------

/// Resolve token symbol to address for a given chain.
pub fn resolve_token(chain_id: u64, symbol_or_addr: &str) -> Option<(String, u8)> {
    // If it starts with 0x, it's already an address
    if symbol_or_addr.starts_with("0x") {
        return Some((symbol_or_addr.to_string(), 18)); // assume 18 decimals
    }

    let s = symbol_or_addr.to_uppercase();
    match (chain_id, s.as_str()) {
        // Arbitrum
        (42161, "USDC") => Some(("0xaf88d065e77c8cC2239327C5EDb3A432268e5831".into(), 6)),
        (42161, "USDT") => Some(("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9".into(), 6)),
        (42161, "WETH" | "ETH") => Some(("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".into(), 18)),
        // Polygon
        (137, "USDC") => Some(("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359".into(), 6)),
        (137, "WMATIC" | "WPOL") => Some(("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270".into(), 18)),
        (137, "NATIVE" | "POL" | "MATIC") => Some(("native".into(), 18)),
        // Ethereum
        (1, "USDC") => Some(("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".into(), 6)),
        (1, "WETH" | "ETH") => Some(("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".into(), 18)),
        // Base
        (8453, "USDC") => Some(("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(), 6)),
        _ => None,
    }
}

/// Build ERC-20 approve calldata.
pub fn build_approve_calldata(spender: &str, amount_raw: &str) -> String {
    let spender_clean = spender.strip_prefix("0x").unwrap_or(spender).to_lowercase();
    let amount_u256 = if amount_raw == "MAX" {
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string()
    } else {
        let val = amount_raw.parse::<u128>().unwrap_or(0);
        format!("{:064x}", val)
    };
    format!(
        "0x095ea7b3000000000000000000000000{spender_clean}{amount_u256}"
    )
}

/// Convert human amount to raw units.
pub fn to_raw(amount: &str, decimals: u8) -> u128 {
    let parts: Vec<&str> = amount.split('.').collect();
    let whole: u128 = parts[0].parse().unwrap_or(0);
    let frac: u128 = if parts.len() > 1 {
        let frac_str = parts[1];
        let padded = format!("{:0<width$}", frac_str, width = decimals as usize);
        padded[..decimals as usize].parse().unwrap_or(0)
    } else {
        0
    };
    whole * 10u128.pow(decimals as u32) + frac
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- to_raw --

    #[test]
    fn to_raw_whole_number() {
        assert_eq!(to_raw("10", 6), 10_000_000);
        assert_eq!(to_raw("1", 18), 1_000_000_000_000_000_000);
    }

    #[test]
    fn to_raw_with_decimals() {
        assert_eq!(to_raw("5.5", 6), 5_500_000);
        assert_eq!(to_raw("0.000001", 6), 1);
        assert_eq!(to_raw("1.23", 6), 1_230_000);
    }

    #[test]
    fn to_raw_zero() {
        assert_eq!(to_raw("0", 6), 0);
        assert_eq!(to_raw("0.0", 18), 0);
    }

    #[test]
    fn to_raw_truncates_excess_decimals() {
        // "1.1234567" with 6 decimals → should take first 6 frac digits
        assert_eq!(to_raw("1.1234567", 6), 1_123_456);
    }

    // -- build_approve_calldata --

    #[test]
    fn approve_calldata_max() {
        let cd = build_approve_calldata("0xDEAD", "MAX");
        assert!(cd.starts_with("0x095ea7b3"));
        // spender left-padded to 32 bytes (24 zeros + "dead")
        assert!(cd.contains("000000000000000000000000dead"), "calldata: {cd}");
        // max uint256
        assert!(cd.ends_with("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"));
    }

    #[test]
    fn approve_calldata_specific_amount() {
        let cd = build_approve_calldata("0xabcd1234abcd1234abcd1234abcd1234abcd1234", "1000000");
        assert!(cd.starts_with("0x095ea7b3"));
        // amount = 1000000 = 0xF4240
        assert!(cd.ends_with(&format!("{:064x}", 1_000_000u128)));
    }

    #[test]
    fn approve_calldata_strips_0x_prefix() {
        let with_prefix = build_approve_calldata("0xABCD", "100");
        let without_prefix = build_approve_calldata("ABCD", "100");
        assert_eq!(with_prefix, without_prefix);
    }

    #[test]
    fn approve_calldata_lowercases_spender() {
        let cd = build_approve_calldata("0xABCDEF", "100");
        assert!(cd.contains("abcdef"));
        assert!(!cd.contains("ABCDEF"));
    }

    // -- resolve_token --

    #[test]
    fn resolve_token_by_symbol() {
        let (addr, dec) = resolve_token(42161, "USDC").unwrap();
        assert!(addr.starts_with("0x"));
        assert_eq!(dec, 6);
    }

    #[test]
    fn resolve_token_case_insensitive() {
        let upper = resolve_token(42161, "USDC");
        let lower = resolve_token(42161, "usdc");
        assert_eq!(upper, lower);
    }

    #[test]
    fn resolve_token_address_passthrough() {
        let addr = "0xDEADBEEF1234567890abcdef1234567890abcdef";
        let (resolved, dec) = resolve_token(42161, addr).unwrap();
        assert_eq!(resolved, addr);
        assert_eq!(dec, 18); // default decimals for raw address
    }

    #[test]
    fn resolve_token_polygon_native() {
        let (addr, dec) = resolve_token(137, "POL").unwrap();
        assert_eq!(addr, "native");
        assert_eq!(dec, 18);

        let (addr2, _) = resolve_token(137, "MATIC").unwrap();
        assert_eq!(addr2, "native");
    }

    #[test]
    fn resolve_token_unknown_returns_none() {
        assert!(resolve_token(42161, "NONEXISTENT").is_none());
        assert!(resolve_token(999, "USDC").is_none());
    }

    #[test]
    fn resolve_token_eth_alias_on_arbitrum() {
        let weth = resolve_token(42161, "WETH");
        let eth = resolve_token(42161, "ETH");
        assert_eq!(weth, eth);
    }

    // -- known_contract --

    #[test]
    fn known_contract_arbitrum_usdc() {
        let (name, risk) =
            known_contract(42161, "0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap();
        assert_eq!(name, "USDC");
        assert_eq!(risk, "low");
    }

    #[test]
    fn known_contract_case_insensitive() {
        let upper =
            known_contract(42161, "0xAF88D065E77C8CC2239327C5EDB3A432268E5831");
        let lower =
            known_contract(42161, "0xaf88d065e77c8cc2239327c5edb3a432268e5831");
        assert_eq!(upper, lower);
    }

    #[test]
    fn known_contract_unknown_returns_none() {
        assert!(known_contract(42161, "0x0000000000000000000000000000000000000000").is_none());
        assert!(known_contract(999, "0xaf88d065e77c8cc2239327c5edb3a432268e5831").is_none());
    }

    #[test]
    fn known_contract_polygon_ctf_exchange() {
        let (name, _) =
            known_contract(137, "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E").unwrap();
        assert_eq!(name, "Polymarket CTF Exchange");
    }

    // -- request/response serialization --

    #[test]
    fn defi_bridge_request_roundtrips_json() {
        let req = DefiBridgeRequest {
            from_chain: 42161,
            to_chain: 137,
            token: "USDC".into(),
            amount: "5.0".into(),
            output_token: Some("native".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: DefiBridgeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.from_chain, 42161);
        assert_eq!(back.output_token.as_deref(), Some("native"));
    }

    #[test]
    fn defi_bridge_request_output_token_defaults_none() {
        let json = r#"{"from_chain":42161,"to_chain":137,"token":"USDC","amount":"5"}"#;
        let req: DefiBridgeRequest = serde_json::from_str(json).unwrap();
        assert!(req.output_token.is_none());
    }

    #[test]
    fn defi_approve_request_amount_defaults_none() {
        let json = r#"{"chain_id":137,"token":"0x00","spender":"0x01"}"#;
        let req: DefiApproveRequest = serde_json::from_str(json).unwrap();
        assert!(req.amount.is_none());
    }

    #[test]
    fn json_schema_generation_succeeds() {
        let _ = schemars::schema_for!(DefiBridgeRequest);
        let _ = schemars::schema_for!(DefiApproveRequest);
        let _ = schemars::schema_for!(DefiAnalyzeRequest);
        let _ = schemars::schema_for!(DefiBridgeResponse);
        let _ = schemars::schema_for!(DefiApproveResponse);
        let _ = schemars::schema_for!(DefiAnalyzeResponse);
    }
}
