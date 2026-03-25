//! Live integration test for `WaiaasSignerBridge` against a real WAIaaS instance.
//!
//! This test is `#[ignore]` by default so it never blocks CI.
//!
//! Run manually with env vars set:
//! ```sh
//! WAIAAS_BASE_URL=https://... \
//! WAIAAS_HOT_SESSION_TOKEN=... \
//! WAIAAS_WALLET_ID=... \
//! WAIAAS_NETWORK=arbitrum \
//! cargo test --manifest-path daemon/Cargo.toml -p a2ex-waiaas-signer \
//!   -- --ignored waiaas_signer_bridge_signs_eip712
//! ```

use a2ex_signer_bridge::{Eip712Domain, SignerBridge, TypedDataSignRequest};
use a2ex_waiaas_signer::WaiaasSignerBridge;
use serde_json::json;

/// Read a required env var or skip the test with a clear message.
fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "Skipping live WAIaaS test: env var `{name}` is not set. \
             Set WAIAAS_BASE_URL, WAIAAS_HOT_SESSION_TOKEN, WAIAAS_WALLET_ID, \
             and WAIAAS_NETWORK to run this test."
        )
    })
}

#[tokio::test]
#[ignore]
async fn waiaas_signer_bridge_signs_eip712() {
    let base_url = required_env("WAIAAS_BASE_URL");
    let token = required_env("WAIAAS_HOT_SESSION_TOKEN");
    let wallet_id = required_env("WAIAAS_WALLET_ID");
    let network = required_env("WAIAAS_NETWORK");

    let bridge = WaiaasSignerBridge::new(&base_url, &token, &wallet_id, &network);

    // Minimal but valid EIP-712 payload
    let req = TypedDataSignRequest {
        payload: vec![],
        domain: Some(Eip712Domain {
            name: Some("A2EX Test".into()),
            version: Some("1".into()),
            chain_id: Some(42161),
            verifying_contract: Some("0x0000000000000000000000000000000000000001".into()),
        }),
        types: Some(json!({
            "Test": [
                { "name": "value", "type": "uint256" }
            ]
        })),
        primary_type: Some("Test".into()),
        message: Some(json!({
            "value": "1"
        })),
    };

    let result = bridge.sign_typed_data(req).await;

    match &result {
        Ok(signed) => {
            let sig = signed
                .signature_hex
                .as_deref()
                .expect("signature_hex should be present");
            println!("WAIaaS live signature: {sig}");

            assert!(
                sig.starts_with("0x"),
                "signature should start with 0x, got: {sig}"
            );
            assert_eq!(
                sig.len(),
                132,
                "signature should be 132 hex chars (65 bytes = r + s + v), got {} chars: {sig}",
                sig.len()
            );
        }
        Err(e) => {
            panic!("WAIaaS sign_typed_data failed: {e:?}");
        }
    }
}
