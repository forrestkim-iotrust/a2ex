//! Integration tests for env-var-based venue adapter wiring in
//! [`build_server_from_env`].
//!
//! Proves both code paths:
//! 1. **Configured:** all required env vars set → server has venue adapters.
//! 2. **Default fallback:** missing required env vars → server starts without
//!    adapters, venue tools return `VenueAdaptersNotConfigured`.
//!
//! # Safety
//!
//! These tests mutate process-global environment variables.  They must run
//! single-threaded (`--test-threads=1`) to avoid data races.

use a2ex_mcp::build_server_from_env;
use std::sync::Mutex;

/// Global mutex to serialize tests that mutate process-global env vars.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

const REQUIRED_VARS: [&str; 4] = [
    "A2EX_WAIAAS_BASE_URL",
    "A2EX_HOT_SESSION_TOKEN",
    "A2EX_HOT_WALLET_ID",
    "A2EX_WAIAAS_NETWORK",
];

const OPTIONAL_VARS: [&str; 4] = [
    "A2EX_ACROSS_INTEGRATOR_ID",
    "A2EX_ACROSS_API_KEY",
    "A2EX_HYPERLIQUID_BASE_URL",
    "A2EX_POLYMARKET_CLOB_BASE_URL",
];

/// Remove all venue-related env vars.
///
/// # Safety
/// Caller must ensure no other threads read these env vars concurrently.
unsafe fn clear_all_venue_env_vars() {
    for var in REQUIRED_VARS.iter().chain(OPTIONAL_VARS.iter()) {
        unsafe { std::env::remove_var(var) };
    }
}

/// When all four required env vars are set, `build_server_from_env` produces
/// a server whose `venue_adapters` lock contains `Some(...)`.
#[test]
fn configured_when_required_env_vars_present() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // SAFETY: serialized by ENV_MUTEX.
    unsafe {
        clear_all_venue_env_vars();
        std::env::set_var("A2EX_WAIAAS_BASE_URL", "http://localhost:9999");
        std::env::set_var("A2EX_HOT_SESSION_TOKEN", "test-token");
        std::env::set_var("A2EX_HOT_WALLET_ID", "test-wallet");
        std::env::set_var("A2EX_WAIAAS_NETWORK", "testnet");
    }

    let server = build_server_from_env();

    let guard = server
        .venue_adapters()
        .read()
        .expect("venue_adapters read lock");
    assert!(
        guard.is_some(),
        "venue adapters should be Some when all required env vars are set"
    );

    unsafe { clear_all_venue_env_vars() };
}

/// When any required env var is missing, `build_server_from_env` falls back
/// to `Default` — venue adapters are `None` but the server is operational.
#[test]
fn fallback_when_required_env_vars_missing() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // SAFETY: serialized by ENV_MUTEX.
    unsafe { clear_all_venue_env_vars() };

    let server = build_server_from_env();

    let guard = server
        .venue_adapters()
        .read()
        .expect("venue_adapters read lock");
    assert!(
        guard.is_none(),
        "venue adapters should be None when required env vars are missing"
    );
}

/// Empty strings are treated as absent — the server falls back to default.
#[test]
fn empty_env_vars_treated_as_missing() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // SAFETY: serialized by ENV_MUTEX.
    unsafe {
        clear_all_venue_env_vars();
        std::env::set_var("A2EX_WAIAAS_BASE_URL", "http://localhost:9999");
        std::env::set_var("A2EX_HOT_SESSION_TOKEN", ""); // empty → treated as missing
        std::env::set_var("A2EX_HOT_WALLET_ID", "test-wallet");
        std::env::set_var("A2EX_WAIAAS_NETWORK", "testnet");
    }

    let server = build_server_from_env();

    let guard = server
        .venue_adapters()
        .read()
        .expect("venue_adapters read lock");
    assert!(
        guard.is_none(),
        "empty env var should be treated as missing → fallback to default"
    );

    unsafe { clear_all_venue_env_vars() };
}

/// Optional env vars (Across, Hyperliquid, Polymarket URLs) are respected
/// when set.  Verify the server still constructs successfully with all
/// optional vars present alongside required vars.
#[test]
fn optional_env_vars_accepted() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // SAFETY: serialized by ENV_MUTEX.
    unsafe {
        clear_all_venue_env_vars();
        std::env::set_var("A2EX_WAIAAS_BASE_URL", "http://localhost:9999");
        std::env::set_var("A2EX_HOT_SESSION_TOKEN", "test-token");
        std::env::set_var("A2EX_HOT_WALLET_ID", "test-wallet");
        std::env::set_var("A2EX_WAIAAS_NETWORK", "mainnet");
        std::env::set_var("A2EX_ACROSS_INTEGRATOR_ID", "int-123");
        std::env::set_var("A2EX_ACROSS_API_KEY", "key-456");
        std::env::set_var("A2EX_HYPERLIQUID_BASE_URL", "http://localhost:8888");
        std::env::set_var("A2EX_POLYMARKET_CLOB_BASE_URL", "http://localhost:7777");
    }

    let server = build_server_from_env();

    let guard = server
        .venue_adapters()
        .read()
        .expect("venue_adapters read lock");
    assert!(guard.is_some(), "should be configured with all vars set");

    unsafe { clear_all_venue_env_vars() };
}
