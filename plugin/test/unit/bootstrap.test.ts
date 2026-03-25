import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mkdtemp, readFile, rm, mkdir } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

// ---------------------------------------------------------------------------
// Mocks — vi.hoisted for shared refs, vi.mock for module replacement
// ---------------------------------------------------------------------------

const { mockStartWaiaas, mockIsWaiaasRunning, mockCreateWaiaasClient } =
  vi.hoisted(() => ({
    mockStartWaiaas: vi.fn(),
    mockIsWaiaasRunning: vi.fn(),
    mockCreateWaiaasClient: vi.fn(),
  }));

vi.mock("../../src/services/waiaas.service.js", () => ({
  startWaiaas: mockStartWaiaas,
  isWaiaasRunning: mockIsWaiaasRunning,
}));

vi.mock("../../src/transport/waiaas-http-client.js", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../src/transport/waiaas-http-client.js")>();
  return {
    ...orig,
    createWaiaasClient: mockCreateWaiaasClient,
  };
});

// Import under test — after mocks
import { createBootstrapTool } from "../../src/tools/bootstrap.js";
import { writeState, type A2exPluginState } from "../../src/state/plugin-state.js";
import { WaiaasApiError } from "../../src/transport/waiaas-http-client.js";
import { STATE_SUBDIR, STATE_FILENAME } from "../../src/constants.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Build a mock WaiaasClient with controllable return values. */
function buildMockClient() {
  return {
    health: vi.fn().mockResolvedValue({ status: "ok", version: "1.0.0" }),
    createWallet: vi.fn()
      .mockResolvedValueOnce({ id: "vault-wallet-id", publicKey: "0xVAULT" })
      .mockResolvedValueOnce({ id: "hot-wallet-id", publicKey: "0xHOT" }),
    createSession: vi.fn()
      .mockResolvedValueOnce({ id: "vault-session-id", token: "vault-token" })
      .mockResolvedValueOnce({ id: "hot-session-id", token: "hot-token" }),
    createPolicy: vi.fn()
      .mockResolvedValueOnce({ id: "vault-policy-id" })
      .mockResolvedValueOnce({ id: "hot-policy-id" })
      .mockResolvedValueOnce({ id: "hot-whitelist-arb-id" })
      .mockResolvedValueOnce({ id: "hot-whitelist-poly-id" }),
  };
}

function readStateFile(stateDir: string): Promise<A2exPluginState> {
  const filePath = join(stateDir, STATE_SUBDIR, STATE_FILENAME);
  return readFile(filePath, "utf-8").then((raw) => JSON.parse(raw));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("bootstrap tool", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-bootstrap-test-"));
    vi.clearAllMocks();

    // Default mock behaviors
    mockStartWaiaas.mockResolvedValue({ pid: 12345, port: 3100 });
    mockIsWaiaasRunning.mockReturnValue(false);
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true }).catch(() => {});
  });

  it("full bootstrap — all 8 steps in order", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    const result = await tool.execute("test", { masterPassword: "test-password" });

    // Step 1: WAIaaS started
    expect(mockStartWaiaas).toHaveBeenCalledOnce();
    expect(mockStartWaiaas).toHaveBeenCalledWith(
      expect.objectContaining({
        masterPassword: "test-password",
        port: 3100,
      }),
    );

    // Steps 2-3: Two wallets created with correct params
    expect(mockClient.createWallet).toHaveBeenCalledTimes(2);
    expect(mockClient.createWallet).toHaveBeenNthCalledWith(
      1,
      { mode: "master", masterPassword: "test-password" },
      { name: "a2ex-vault", chain: "ethereum", environment: "mainnet" },
    );
    expect(mockClient.createWallet).toHaveBeenNthCalledWith(
      2,
      { mode: "master", masterPassword: "test-password" },
      { name: "a2ex-hot", chain: "ethereum", environment: "mainnet" },
    );

    // Steps 4-5: Two sessions created
    expect(mockClient.createSession).toHaveBeenCalledTimes(2);

    // Steps 6-7: Two policies created
    expect(mockClient.createPolicy).toHaveBeenCalledTimes(4);

    // Result envelope
    const parsed = JSON.parse((result as any).content[0].text);
    expect(parsed).toEqual({
      status: "bootstrapped",
      vaultAddress: "0xVAULT",
      hotAddress: "0xHOT",
      fundingRequired: "Send ETH + USDC to vault address",
      a2exSpawn: "skipped",
    });
  });

  it("session scope — vault gets [vaultId, hotId], hot gets [hotId] only", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "test-password" });

    // Vault session: scoped to both wallets
    expect(mockClient.createSession).toHaveBeenNthCalledWith(
      1,
      { mode: "master", masterPassword: "test-password" },
      { walletIds: ["vault-wallet-id", "hot-wallet-id"] },
    );

    // Hot session: scoped to hot wallet only
    expect(mockClient.createSession).toHaveBeenNthCalledWith(
      2,
      { mode: "master", masterPassword: "test-password" },
      { walletIds: ["hot-wallet-id"] },
    );
  });

  it("vault policy: SPENDING_LIMIT, instant_max_usd:0, delay_max_usd:10", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "test-password" });

    expect(mockClient.createPolicy).toHaveBeenNthCalledWith(
      1,
      { mode: "master", masterPassword: "test-password" },
      {
        walletId: "vault-wallet-id",
        type: "SPENDING_LIMIT",
        rules: { instant_max_usd: 0, delay_max_usd: 10 },
      },
    );
  });

  it("hot policy: SPENDING_LIMIT, instant_max_usd:50", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "test-password" });

    expect(mockClient.createPolicy).toHaveBeenNthCalledWith(
      2,
      { mode: "master", masterPassword: "test-password" },
      {
        walletId: "hot-wallet-id",
        type: "SPENDING_LIMIT",
        rules: { instant_max_usd: 50 },
      },
    );
  });

  it("idempotency — existing vaultWalletId → only hot wallet created", async () => {
    // Pre-seed state with vault wallet already created
    const existingState: A2exPluginState = {
      phase: "bootstrapping",
      waiaasPort: 3100,
      waiaasPid: 99999,
      vaultWalletId: "existing-vault-id",
      vaultAddress: "0xEXISTING_VAULT",
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, existingState);

    // WAIaaS already running
    mockIsWaiaasRunning.mockReturnValue(true);

    const mockClient = buildMockClient();
    // Only one wallet call expected (hot)
    mockClient.createWallet = vi.fn().mockResolvedValueOnce({
      id: "hot-wallet-id",
      publicKey: "0xHOT",
    });
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "test-password" });

    // WAIaaS not started again (already running)
    expect(mockStartWaiaas).not.toHaveBeenCalled();

    // Only one wallet created (hot)
    expect(mockClient.createWallet).toHaveBeenCalledOnce();
    expect(mockClient.createWallet).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ name: "a2ex-hot" }),
    );

    // Sessions and policies still created
    expect(mockClient.createSession).toHaveBeenCalledTimes(2);
    expect(mockClient.createPolicy).toHaveBeenCalledTimes(4);

    // Vault session uses existing vault ID + new hot ID
    expect(mockClient.createSession).toHaveBeenNthCalledWith(
      1,
      expect.anything(),
      { walletIds: ["existing-vault-id", "hot-wallet-id"] },
    );
  });

  it("partial failure recovery — wallets exist, no sessions → only session+policy calls", async () => {
    const existingState: A2exPluginState = {
      phase: "bootstrapping",
      waiaasPort: 3100,
      waiaasPid: 99999,
      vaultWalletId: "vault-id",
      vaultAddress: "0xVAULT",
      hotWalletId: "hot-id",
      hotAddress: "0xHOT",
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, existingState);
    mockIsWaiaasRunning.mockReturnValue(true);

    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "test-password" });

    // No wallet or WAIaaS calls
    expect(mockStartWaiaas).not.toHaveBeenCalled();
    expect(mockClient.createWallet).not.toHaveBeenCalled();

    // Sessions and policies made
    expect(mockClient.createSession).toHaveBeenCalledTimes(2);
    expect(mockClient.createPolicy).toHaveBeenCalledTimes(4);
  });

  it("masterPassword is persisted in state for WAIaaS restart & key recovery", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "super-secret-password-123" });

    const stateRaw = await readFile(
      join(stateDir, STATE_SUBDIR, STATE_FILENAME),
      "utf-8",
    );

    const state = JSON.parse(stateRaw) as A2exPluginState;
    expect(state.phase).toBe("bootstrapped");
    expect(state.masterPassword).toBe("super-secret-password-123");
  });

  it("error propagation — WaiaasApiError surfaces in tool result", async () => {
    const mockClient = buildMockClient();
    mockClient.createWallet.mockReset();
    mockClient.createWallet.mockRejectedValue(
      new WaiaasApiError(400, "WALLET_EXISTS", "Wallet already exists"),
    );
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);

    await expect(tool.execute("test", { masterPassword: "test-password" })).rejects.toThrow(
      WaiaasApiError,
    );
  });

  it("returns error when stateDir is null", async () => {
    const tool = createBootstrapTool(() => null);
    const result = await tool.execute("test", { masterPassword: "test-password" });

    const parsed = JSON.parse((result as any).content[0].text);
    expect(parsed.error).toContain("stateDir unavailable");
    expect((result as any).isError).toBe(true);
  });

  it("uses default masterPassword when not provided", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    const result = await tool.execute("test", {});

    expect((result as any).isError).toBeFalsy();
  });

  it("writes intermediate state after each step for crash safety", async () => {
    // Simulate a failure mid-bootstrap (after vault wallet, before hot wallet)
    const mockClient = buildMockClient();
    mockClient.createWallet = vi.fn()
      .mockResolvedValueOnce({ id: "vault-wallet-id", publicKey: "0xVAULT" })
      .mockRejectedValueOnce(new WaiaasApiError(500, "INTERNAL", "server down"));
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await expect(tool.execute("test", { masterPassword: "test-password" })).rejects.toThrow();

    // Even though bootstrap failed, vault wallet should be persisted
    const state = await readStateFile(stateDir);
    expect(state.phase).toBe("bootstrapping");
    expect(state.vaultWalletId).toBe("vault-wallet-id");
    expect(state.vaultAddress).toBe("0xVAULT");
    // Hot wallet was not created
    expect(state.hotWalletId).toBeUndefined();
  });

  it("creates config.toml with RPC URLs after WAIaaS init", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "test-password" });

    // config.toml should exist in waiaas data dir
    const dataDir = join(stateDir, STATE_SUBDIR, "waiaas-data");
    await mkdir(dataDir, { recursive: true }).catch(() => {}); // ensure dir exists for read
    const configPath = join(dataDir, "config.toml");
    const content = await readFile(configPath, "utf-8");

    expect(content).toContain("evm_polygon_mainnet");
    expect(content).toContain("polygon-bor-rpc.publicnode.com");
    expect(content).toContain("evm_arbitrum_mainnet");
    expect(content).toContain("arb1.arbitrum.io");
  });

  it("config.toml is idempotent — no duplicate on re-run", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    // First run
    await tool.execute("test", { masterPassword: "pw1" });

    // Reset mocks for second run (needs fresh mock responses)
    mockIsWaiaasRunning.mockReturnValue(true);
    mockCreateWaiaasClient.mockReturnValue(buildMockClient());

    // Write state to simulate already-bootstrapped
    const state = await readStateFile(stateDir);
    const { writeState: ws } = await import("../../src/state/plugin-state.js");
    await ws(stateDir, { ...state, phase: "not_initialized" });

    // Second run
    await tool.execute("test", { masterPassword: "pw2" });

    const dataDir = join(stateDir, STATE_SUBDIR, "waiaas-data");
    const content = await readFile(join(dataDir, "config.toml"), "utf-8");
    // Should only contain RPC block once
    const matches = content.match(/evm_polygon_mainnet/g);
    expect(matches).toHaveLength(1);
  });
});
