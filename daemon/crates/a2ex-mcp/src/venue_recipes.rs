//! Layer 3: Venue Recipes — high-level trading tools.
//!
//! These compose Layer 1 (chain) + Layer 2 (defi) primitives into
//! one-shot trading actions. AI calls one tool, everything else is automatic.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool name constants
// ---------------------------------------------------------------------------

pub const TOOL_POLYMARKET_TRADE: &str = "polymarket_trade";
pub const TOOL_HYPERLIQUID_TRADE: &str = "hyperliquid_trade";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Place an order on Polymarket. Handles bridge, gas, approval, credentials, and order signing automatically.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolymarketTradeRequest {
    /// Polymarket condition token ID.
    pub token_id: String,
    /// Order side: "buy" or "sell".
    pub side: String,
    /// Number of outcome tokens.
    pub size: String,
    /// Limit price per token (0.0–1.0).
    pub price: String,
    /// Whether this is a neg-risk market. Defaults to false.
    #[serde(default)]
    pub neg_risk: bool,
}

/// Place an order on Hyperliquid perpetual exchange. Handles deposit and order signing automatically.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HyperliquidTradeRequest {
    /// Asset symbol (e.g. "ETH", "BTC").
    pub asset: String,
    /// True for buy/long, false for sell/short.
    pub is_buy: bool,
    /// Order size in base units.
    pub size: String,
    /// Limit price.
    pub price: String,
    /// Order type: "limit" or "market". Defaults to "limit".
    #[serde(default = "default_limit")]
    pub order_type: String,
}

fn default_limit() -> String {
    "limit".to_string()
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VenueTradeResponse {
    /// Venue name.
    pub venue: String,
    /// Order ID from the venue.
    #[serde(default)]
    pub order_id: Option<String>,
    /// Order status: "submitted", "filled", "rejected", "failed".
    pub status: String,
    /// Human-readable cost (e.g. "$0.50").
    #[serde(default)]
    pub cost: Option<String>,
    /// Error message if failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Steps executed (for transparency).
    pub steps: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_constants_unique() {
        assert_ne!(TOOL_POLYMARKET_TRADE, TOOL_HYPERLIQUID_TRADE);
        assert!(TOOL_POLYMARKET_TRADE.contains("polymarket"));
        assert!(TOOL_HYPERLIQUID_TRADE.contains("hyperliquid"));
    }

    #[test]
    fn polymarket_request_neg_risk_defaults_false() {
        let json = r#"{"token_id":"abc","side":"buy","size":"10","price":"0.5"}"#;
        let req: PolymarketTradeRequest = serde_json::from_str(json).unwrap();
        assert!(!req.neg_risk);
    }

    #[test]
    fn hyperliquid_request_order_type_defaults_limit() {
        let json = r#"{"asset":"ETH","is_buy":true,"size":"0.1","price":"3000"}"#;
        let req: HyperliquidTradeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.order_type, "limit");
    }

    #[test]
    fn hyperliquid_request_with_market_order_type() {
        let json = r#"{"asset":"BTC","is_buy":false,"size":"0.01","price":"60000","order_type":"market"}"#;
        let req: HyperliquidTradeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.order_type, "market");
    }

    #[test]
    fn venue_trade_response_optional_fields_default_none() {
        let json = r#"{"venue":"polymarket","status":"submitted","steps":["bridge","approve"]}"#;
        let resp: VenueTradeResponse = serde_json::from_str(json).unwrap();
        assert!(resp.order_id.is_none());
        assert!(resp.cost.is_none());
        assert!(resp.error.is_none());
        assert_eq!(resp.steps.len(), 2);
    }

    #[test]
    fn json_schema_generation_succeeds() {
        let _ = schemars::schema_for!(PolymarketTradeRequest);
        let _ = schemars::schema_for!(HyperliquidTradeRequest);
        let _ = schemars::schema_for!(VenueTradeResponse);
    }
}
