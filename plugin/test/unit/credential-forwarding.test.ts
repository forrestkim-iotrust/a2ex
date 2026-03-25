import { describe, it, expect } from "vitest";
import { buildA2exSubprocessEnv } from "../../src/index.js";
import type { A2exPluginState } from "../../src/state/plugin-state.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Minimal valid state — only required fields. */
function baseState(overrides: Partial<A2exPluginState> = {}): A2exPluginState {
  return {
    phase: "running",
    waiaasPort: 8080,
    lastUpdated: new Date().toISOString(),
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("buildA2exSubprocessEnv", () => {
  // ---- Happy path ----

  it("includes all expected A2EX_* vars when state has hot credentials", () => {
    const state = baseState({
      hotSessionToken: "tok_abc123",
      hotWalletId: "wallet_xyz",
      hotAddress: "0xDeadBeef",
      waiaasPort: 9090,
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeDefined();
    expect(env).toEqual({
      A2EX_WAIAAS_BASE_URL: "http://localhost:9090",
      A2EX_HOT_SESSION_TOKEN: "tok_abc123",
      A2EX_HOT_WALLET_ID: "wallet_xyz",
      A2EX_HOT_WALLET_ADDRESS: "0xDeadBeef",
      A2EX_WAIAAS_NETWORK: "arbitrum-mainnet",
    });
  });

  // ---- Security: vault credentials never forwarded ----

  it("never includes vaultSessionToken in env", () => {
    const state = baseState({
      hotSessionToken: "tok_abc123",
      hotWalletId: "wallet_xyz",
      vaultSessionToken: "vault_secret_should_not_appear",
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeDefined();
    expect(env).not.toHaveProperty("vaultSessionToken");
    expect(env).not.toHaveProperty("A2EX_VAULT_SESSION_TOKEN");
    // Ensure the value doesn't appear anywhere in the env object
    const allValues = Object.values(env!);
    expect(allValues).not.toContain("vault_secret_should_not_appear");
  });

  it("never includes masterPassword in env even when present in state", () => {
    const state = baseState({
      hotSessionToken: "tok_abc123",
      hotWalletId: "wallet_xyz",
      masterPassword: "super-secret-master-pw-should-never-leak",
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeDefined();
    expect(env).not.toHaveProperty("masterPassword");
    expect(env).not.toHaveProperty("A2EX_MASTER_PASSWORD");
    // Value must not appear anywhere in env
    const allValues = Object.values(env!);
    expect(allValues).not.toContain("super-secret-master-pw-should-never-leak");
    // Only the expected keys should be present
    expect(Object.keys(env!).sort()).toEqual([
      "A2EX_HOT_SESSION_TOKEN",
      "A2EX_HOT_WALLET_ID",
      "A2EX_WAIAAS_BASE_URL",
      "A2EX_WAIAAS_NETWORK",
    ].sort());
  });

  // ---- Missing state fields → omitted vars ----

  it("returns undefined when no hot credentials are present", () => {
    const state = baseState(); // no hotSessionToken, no hotWalletId

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeUndefined();
  });

  it("returns undefined when hot credentials are empty strings", () => {
    const state = baseState({
      hotSessionToken: "",
      hotWalletId: "",
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeUndefined();
  });

  it("omits A2EX_HOT_WALLET_ID when hotWalletId is missing", () => {
    const state = baseState({
      hotSessionToken: "tok_abc123",
      // hotWalletId not set
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeDefined();
    expect(env).toHaveProperty("A2EX_HOT_SESSION_TOKEN", "tok_abc123");
    expect(env).not.toHaveProperty("A2EX_HOT_WALLET_ID");
  });

  it("omits A2EX_HOT_SESSION_TOKEN when hotSessionToken is missing", () => {
    const state = baseState({
      hotWalletId: "wallet_xyz",
      // hotSessionToken not set
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeDefined();
    expect(env).toHaveProperty("A2EX_HOT_WALLET_ID", "wallet_xyz");
    expect(env).not.toHaveProperty("A2EX_HOT_SESSION_TOKEN");
  });

  // ---- Default network ----

  it("sets A2EX_WAIAAS_NETWORK to arbitrum-mainnet by default", () => {
    const state = baseState({
      hotSessionToken: "tok_abc123",
      hotWalletId: "wallet_xyz",
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeDefined();
    expect(env!.A2EX_WAIAAS_NETWORK).toBe("arbitrum-mainnet");
  });

  // ---- waiaasPort handling ----

  it("builds correct base URL from waiaasPort", () => {
    const state = baseState({
      hotSessionToken: "tok_abc123",
      waiaasPort: 3456,
    });

    const env = buildA2exSubprocessEnv(state);

    expect(env).toBeDefined();
    expect(env!.A2EX_WAIAAS_BASE_URL).toBe("http://localhost:3456");
  });
});
