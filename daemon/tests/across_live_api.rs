//! Live Across API smoke test — calls the real Across Swap API.
//!
//! This test is `#[ignore]` by default. Run with:
//!   cargo test -p a2ex-across-adapter --test across_live_api -- --ignored --nocapture
//!
//! Cost: $0 (read-only API call, no on-chain tx)

use a2ex_across_adapter::{AcrossBridgeQuoteRequest, AcrossTransport};
use a2ex_across_adapter::transport::AcrossHttpTransport;

const FUNDING_WALLET: &str = "0x02Ec7337Bb67D5a0B564A1485b4eB90B7d546EE2";
const USDC_ARBITRUM: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
const USDC_POLYGON: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";

#[tokio::test]
#[ignore] // requires network access
async fn across_live_quote_arbitrum_to_polygon_usdc() {
    let transport = AcrossHttpTransport::new(
        "https://app.across.to/api",
        None,
        None,
    );

    let request = AcrossBridgeQuoteRequest {
        asset: USDC_ARBITRUM.to_string(),
        amount_usd: 100_000, // 0.1 USDC in 6-decimal raw units
        source_chain: "42161".to_string(),
        destination_chain: "137".to_string(),
        depositor: Some(FUNDING_WALLET.to_string()),
        recipient: Some(FUNDING_WALLET.to_string()),
        output_token: Some(USDC_POLYGON.to_string()),
    };

    let result = transport.quote(request).await;

    match result {
        Ok(quote) => {
            println!("=== Across Live Quote SUCCESS ===");
            println!("route_id: {}", quote.route_id);
            println!("calldata present: {}", quote.calldata.is_some());
            println!("swap_tx present: {}", quote.swap_tx.is_some());
            if let Some(ref tx) = quote.swap_tx {
                println!("swap_tx.to: {}", tx.to);
                println!("swap_tx.data length: {} chars", tx.data.len());
            }
            println!("input_amount: {:?}", quote.input_amount);
            println!("output_amount: {:?}", quote.output_amount);
            println!("expected_fill_seconds: {}", quote.expected_fill_seconds);
            println!("bridge_fee_usd: {}", quote.bridge_fee_usd);
            println!("quote_expiry_secs: {:?}", quote.quote_expiry_secs);
            println!("approval.token: {}", quote.approval.token);
            println!("approval.spender: {}", quote.approval.spender);

            // Critical assertions
            assert!(quote.calldata.is_some(), "Expected calldata from Across API");
            assert!(quote.swap_tx.is_some(), "Expected swap_tx from Across API");
            assert!(!quote.route_id.is_empty(), "Expected non-empty route_id");
            assert_eq!(quote.route_id, "across-42161-137");

            // Verify we got real amounts back
            assert!(quote.input_amount.is_some(), "Expected input_amount");
            assert!(quote.output_amount.is_some(), "Expected output_amount");

            // The swap tx should be to a real SpokePool contract
            let tx = quote.swap_tx.as_ref().unwrap();
            assert!(tx.to.starts_with("0x"), "swap_tx.to should be an address");
            assert!(tx.data.len() > 10, "swap_tx.data should have real calldata");
        }
        Err(e) => {
            panic!("Across live quote failed: {e:?}");
        }
    }
}
