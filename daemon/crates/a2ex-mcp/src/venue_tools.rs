use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool name constants — all follow the `venue.*` namespace
// ---------------------------------------------------------------------------

pub const TOOL_VENUE_PREPARE_BRIDGE: &str = "venue.prepare_bridge";
pub const TOOL_VENUE_TRADE_POLYMARKET: &str = "venue.trade_polymarket";
pub const TOOL_VENUE_TRADE_HYPERLIQUID: &str = "venue.trade_hyperliquid";
pub const TOOL_VENUE_QUERY_POSITIONS: &str = "venue.query_positions";
pub const TOOL_VENUE_BRIDGE_STATUS: &str = "venue.bridge_status";
pub const TOOL_VENUE_DERIVE_API_KEY: &str = "venue.derive_api_key";

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Prepare an Across bridge quote and return calldata for local signing.
/// This is a 와리가리 (round-trip) tool: it returns transaction calldata
/// that the caller must submit via `waiaas.call_contract`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PrepareBridgeRequest {
    /// Token symbol or address to bridge (e.g. "USDC").
    pub asset: String,
    /// Amount to bridge in USD.
    pub amount_usd: u64,
    /// Source chain identifier (e.g. "ethereum", "polygon").
    pub source_chain: String,
    /// Destination chain identifier (e.g. "polygon", "arbitrum").
    pub destination_chain: String,
    /// Depositor address (who is sending the funds). Required by Across API.
    #[serde(default)]
    pub depositor: Option<String>,
    /// Recipient address (who receives on destination chain). Defaults to depositor.
    #[serde(default)]
    pub recipient: Option<String>,
    /// Output token address on destination chain. Defaults to same as input token.
    #[serde(default)]
    pub output_token: Option<String>,
}

/// Place an order on Polymarket (prediction market).
/// This is a 직통 (direct) tool: the daemon signs and submits the order
/// internally, returning the final result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TradePolymarketRequest {
    /// The condition token ID identifying the market outcome.
    pub token_id: String,
    /// Wallet address whose derived Polymarket credentials should be used.
    pub wallet_address: String,
    /// Order side: "buy" or "sell".
    pub side: String,
    /// Order size in outcome tokens.
    pub size: String,
    /// Limit price per token (0.0–1.0 range).
    pub price: String,
    /// Order type, e.g. "limit" or "market".
    #[serde(default = "default_order_type")]
    pub order_type: String,
}

/// Place an order on Hyperliquid perpetual exchange.
/// This is a 직통 (direct) tool: the daemon signs and submits the order
/// internally, returning the final result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TradeHyperliquidRequest {
    /// Asset symbol or index (e.g. "ETH", "BTC").
    pub asset: String,
    /// True for a buy/long order, false for sell/short.
    pub is_buy: bool,
    /// Order size in base units.
    pub size: String,
    /// Limit price.
    pub price: String,
    /// Order type, e.g. "limit" or "market".
    #[serde(default = "default_order_type")]
    pub order_type: String,
    /// If true, the order can only reduce an existing position.
    #[serde(default)]
    pub reduce_only: bool,
}

/// Query open positions across configured venues.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryPositionsRequest {
    /// Optional venue filter. If `None`, returns positions from all venues.
    /// Valid values: "polymarket", "hyperliquid".
    #[serde(default)]
    pub venue: Option<String>,
}

/// Check the status of an Across bridge transfer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BridgeStatusRequest {
    /// The deposit ID returned by a prior bridge preparation or submission.
    pub deposit_id: String,
}

/// Derive a venue API key from a wallet address.
/// Used for Polymarket CLOB credential derivation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeriveApiKeyRequest {
    /// The wallet address to derive credentials for.
    pub wallet_address: String,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Swap transaction calldata for local signing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SwapTxEnvelope {
    /// Target contract address.
    pub to: String,
    /// Hex-encoded calldata.
    pub data: String,
    /// ETH value to send (wei, as decimal string).
    pub value: String,
}

/// Approval transaction calldata.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalTxEnvelope {
    /// Target contract address (token).
    pub to: String,
    /// Hex-encoded calldata for the approval.
    pub data: String,
}

/// Quote metadata from the bridge provider.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BridgeQuoteMetadata {
    /// Provider-assigned route identifier.
    pub route_id: String,
    /// Estimated bridge fee in USD.
    pub bridge_fee_usd: u64,
    /// Estimated fill time in seconds.
    pub expected_fill_seconds: u64,
    /// Input amount in token units (string to preserve precision).
    #[serde(default)]
    pub input_amount: Option<String>,
    /// Output amount in token units (string to preserve precision).
    #[serde(default)]
    pub output_amount: Option<String>,
}

/// 와리가리 response — returns calldata + approval metadata for the caller
/// to submit via `waiaas.call_contract`. The daemon does NOT submit this
/// transaction; the caller's local signer handles submission.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PrepareBridgeResponse {
    /// The main swap/deposit transaction to sign and submit.
    pub swap_tx: SwapTxEnvelope,
    /// Any required token approval transactions (sign and submit before `swap_tx`).
    #[serde(default)]
    pub approval_txns: Vec<ApprovalTxEnvelope>,
    /// Chain ID on which to submit transactions.
    pub chain_id: u64,
    /// Quote metadata from the bridge provider.
    pub quote: BridgeQuoteMetadata,
    /// Hint for the caller: submit approvals first, then `swap_tx`, then
    /// check status via `venue.bridge_status`.
    pub next_step: String,
}

/// 직통 response — Polymarket order acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TradePolymarketResponse {
    /// Exchange-assigned order identifier.
    pub order_id: String,
    /// Order status (e.g. "submitted", "filled", "rejected").
    pub status: String,
    /// Amount filled so far (in outcome tokens).
    #[serde(default)]
    pub filled_amount: Option<String>,
    /// Venue name — always "polymarket".
    pub venue: String,
}

/// 직통 response — Hyperliquid order acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TradeHyperliquidResponse {
    /// Exchange-assigned order identifier.
    pub order_id: String,
    /// Order status (e.g. "submitted", "filled", "rejected").
    pub status: String,
    /// Venue name — always "hyperliquid".
    pub venue: String,
}

/// A single position entry from any venue.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PositionEntry {
    /// Venue where the position is held.
    pub venue: String,
    /// Asset symbol or identifier.
    pub asset: String,
    /// Position size (signed: positive = long, negative = short).
    pub size: String,
    /// Average entry price.
    pub entry_price: String,
    /// Unrealized PnL in USD.
    pub pnl: String,
}

/// Response for position queries across venues.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryPositionsResponse {
    /// List of open positions, possibly filtered by venue.
    pub positions: Vec<PositionEntry>,
}

/// Status of an Across bridge transfer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BridgeStatusResponse {
    /// The deposit ID being tracked.
    pub deposit_id: String,
    /// Transfer status (e.g. "pending", "filled", "expired").
    pub status: String,
    /// Fill transaction hash on destination chain, if available.
    #[serde(default)]
    pub fill_tx_hash: Option<String>,
    /// Destination chain transaction ID, if available.
    #[serde(default)]
    pub destination_tx_id: Option<String>,
}

/// Result of API key derivation (no secrets exposed).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeriveApiKeyResponse {
    /// Whether the key derivation succeeded.
    pub success: bool,
    /// Human-readable message (e.g. "API key derived successfully").
    /// Never contains the actual secret.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_order_type() -> String {
    "limit".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_constants_are_unique_venue_namespace() {
        let constants = [
            TOOL_VENUE_PREPARE_BRIDGE,
            TOOL_VENUE_TRADE_POLYMARKET,
            TOOL_VENUE_TRADE_HYPERLIQUID,
            TOOL_VENUE_QUERY_POSITIONS,
            TOOL_VENUE_BRIDGE_STATUS,
            TOOL_VENUE_DERIVE_API_KEY,
        ];
        // All start with "venue."
        for c in &constants {
            assert!(c.starts_with("venue."), "{c} does not start with venue.");
        }
        // All unique
        let mut unique = std::collections::HashSet::new();
        for c in &constants {
            assert!(unique.insert(c), "duplicate tool constant: {c}");
        }
    }

    #[test]
    fn request_types_produce_valid_json_schema() {
        // Serialize schemas to JSON — if JsonSchema derive is broken, this panics
        fn assert_schema_has_properties<T: JsonSchema>(name: &str) {
            let schema = schemars::schema_for!(T);
            let json = serde_json::to_string(&schema).expect("schema serializes");
            assert!(!json.is_empty(), "{name} schema is empty");
            // All request types should mention their type name somewhere in the schema
            assert!(
                json.contains("properties") || json.contains("$ref"),
                "{name} schema has no properties or refs: {json}"
            );
        }
        assert_schema_has_properties::<PrepareBridgeRequest>("PrepareBridgeRequest");
        assert_schema_has_properties::<TradePolymarketRequest>("TradePolymarketRequest");
        assert_schema_has_properties::<TradeHyperliquidRequest>("TradeHyperliquidRequest");
        assert_schema_has_properties::<QueryPositionsRequest>("QueryPositionsRequest");
        assert_schema_has_properties::<BridgeStatusRequest>("BridgeStatusRequest");
        assert_schema_has_properties::<DeriveApiKeyRequest>("DeriveApiKeyRequest");
    }

    #[test]
    fn response_types_produce_valid_json_schema() {
        fn assert_schema_has_properties<T: JsonSchema>(name: &str) {
            let schema = schemars::schema_for!(T);
            let json = serde_json::to_string(&schema).expect("schema serializes");
            assert!(!json.is_empty(), "{name} schema is empty");
            assert!(
                json.contains("properties") || json.contains("$ref"),
                "{name} schema has no properties or refs: {json}"
            );
        }
        assert_schema_has_properties::<PrepareBridgeResponse>("PrepareBridgeResponse");
        assert_schema_has_properties::<TradePolymarketResponse>("TradePolymarketResponse");
        assert_schema_has_properties::<TradeHyperliquidResponse>("TradeHyperliquidResponse");
        assert_schema_has_properties::<QueryPositionsResponse>("QueryPositionsResponse");
        assert_schema_has_properties::<BridgeStatusResponse>("BridgeStatusResponse");
        assert_schema_has_properties::<DeriveApiKeyResponse>("DeriveApiKeyResponse");
    }

    #[test]
    fn prepare_bridge_response_has_warigari_fields() {
        let response = PrepareBridgeResponse {
            swap_tx: SwapTxEnvelope {
                to: "0xabc".to_owned(),
                data: "0x1234".to_owned(),
                value: "0".to_owned(),
            },
            approval_txns: vec![ApprovalTxEnvelope {
                to: "0xtoken".to_owned(),
                data: "0xapprove".to_owned(),
            }],
            chain_id: 137,
            quote: BridgeQuoteMetadata {
                route_id: "route-1".to_owned(),
                bridge_fee_usd: 2,
                expected_fill_seconds: 120,
                input_amount: Some("100000000".to_owned()),
                output_amount: Some("99800000".to_owned()),
            },
            next_step: "submit approvals, then swap_tx via waiaas.call_contract".to_owned(),
        };
        assert_eq!(response.swap_tx.to, "0xabc");
        assert_eq!(response.approval_txns.len(), 1);
        assert_eq!(response.chain_id, 137);
    }

    #[test]
    fn trade_responses_have_jiktong_fields() {
        let poly = TradePolymarketResponse {
            order_id: "pm-123".to_owned(),
            status: "submitted".to_owned(),
            filled_amount: None,
            venue: "polymarket".to_owned(),
        };
        assert_eq!(poly.venue, "polymarket");
        assert_eq!(poly.order_id, "pm-123");

        let hl = TradeHyperliquidResponse {
            order_id: "hl-456".to_owned(),
            status: "filled".to_owned(),
            venue: "hyperliquid".to_owned(),
        };
        assert_eq!(hl.venue, "hyperliquid");
        assert_eq!(hl.order_id, "hl-456");
    }
}
