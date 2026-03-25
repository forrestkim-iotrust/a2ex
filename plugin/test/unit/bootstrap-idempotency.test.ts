import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/** Fully bootstrapped state — all 8 steps complete. */
function fullyBootstrappedState(): A2exPluginState {
  return {
    phase: "bootstrapped",
    waiaasPort: 3100,
    waiaasPid: 11111,
    vaultWalletId: "vault-w",
    vaultAddress: "0xVAULT",
    hotWalletId: "hot-w",
    hotAddress: "0xHOT",
    vaultSessionToken: "vault-tok",
    hotSessionToken: "hot-tok",
    policyIds: { vault: "vault-pol", hot: "hot-pol", hotWhitelistArb: "arb-wl", hotWhitelistPoly: "poly-wl" },
    lastUpdated: new Date().toISOString(),
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("bootstrap idempotency", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-idem-test-"));
    vi.clearAllMocks();
    mockStartWaiaas.mockResolvedValue({ pid: 12345, port: 3100 });
    mockIsWaiaasRunning.mockReturnValue(false);
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true }).catch(() => {});
  });

  it("WAIaaS died mid-bootstrap — restarts WAIaaS, skips vault wallet, continues from hot wallet", async () => {
    // State: WAIaaS was started (pid set), vault wallet created, but WAIaaS crashed
    const partialState: A2exPluginState = {
      phase: "bootstrapping",
      waiaasPort: 3100,
      waiaasPid: 99999, // stale PID
      vaultWalletId: "existing-vault",
      vaultAddress: "0xEXISTING_VAULT",
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, partialState);

    // isWaiaasRunning returns false — the process died
    mockIsWaiaasRunning.mockReturnValue(false);

    const mockClient = buildMockClient();
    // Only hot wallet will be created (vault already exists)
    mockClient.createWallet = vi.fn().mockResolvedValueOnce({
      id: "hot-wallet-id",
      publicKey: "0xHOT",
    });
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    const result = await tool.execute("test", { masterPassword: "pw" });

    // WAIaaS restarted because old PID was dead
    expect(mockStartWaiaas).toHaveBeenCalledOnce();

    // Vault wallet skipped, only hot wallet created
    expect(mockClient.createWallet).toHaveBeenCalledOnce();
    expect(mockClient.createWallet).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ name: "a2ex-hot" }),
    );

    // Sessions and policies still created
    expect(mockClient.createSession).toHaveBeenCalledTimes(2);
    expect(mockClient.createPolicy).toHaveBeenCalledTimes(4);

    // Final result is successful
    const parsed = JSON.parse((result as any).content[0].text);
    expect(parsed.status).toBe("bootstrapped");
  });

  it("all 8 steps complete (phase=bootstrapped) — re-bootstrap is no-op", async () => {
    await writeState(stateDir, fullyBootstrappedState());

    // WAIaaS is running
    mockIsWaiaasRunning.mockReturnValue(true);

    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    const result = await tool.execute("test", { masterPassword: "pw" });

    // No API calls at all
    expect(mockStartWaiaas).not.toHaveBeenCalled();
    expect(mockClient.createWallet).not.toHaveBeenCalled();
    expect(mockClient.createSession).not.toHaveBeenCalled();
    expect(mockClient.createPolicy).not.toHaveBeenCalled();

    // Returns current state
    const parsed = JSON.parse((result as any).content[0].text);
    expect(parsed.status).toBe("bootstrapped");
    expect(parsed.vaultAddress).toBe("0xVAULT");
    expect(parsed.hotAddress).toBe("0xHOT");
  });

  it("phase=bootstrapping with partial state (wallets done, no sessions) — resumes from sessions", async () => {
    const partialState: A2exPluginState = {
      phase: "bootstrapping",
      waiaasPort: 3100,
      waiaasPid: 22222,
      vaultWalletId: "vault-id",
      vaultAddress: "0xVAULT",
      hotWalletId: "hot-id",
      hotAddress: "0xHOT",
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, partialState);
    mockIsWaiaasRunning.mockReturnValue(true);

    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "pw" });

    // No WAIaaS start, no wallet calls
    expect(mockStartWaiaas).not.toHaveBeenCalled();
    expect(mockClient.createWallet).not.toHaveBeenCalled();

    // Sessions and policies created
    expect(mockClient.createSession).toHaveBeenCalledTimes(2);
    expect(mockClient.createPolicy).toHaveBeenCalledTimes(4);
  });

  it("phase=bootstrapping with sessions done but no policies — resumes from policies only", async () => {
    const partialState: A2exPluginState = {
      phase: "bootstrapping",
      waiaasPort: 3100,
      waiaasPid: 33333,
      vaultWalletId: "vault-id",
      vaultAddress: "0xVAULT",
      hotWalletId: "hot-id",
      hotAddress: "0xHOT",
      vaultSessionToken: "vs-tok",
      hotSessionToken: "hs-tok",
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, partialState);
    mockIsWaiaasRunning.mockReturnValue(true);

    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "pw" });

    expect(mockStartWaiaas).not.toHaveBeenCalled();
    expect(mockClient.createWallet).not.toHaveBeenCalled();
    expect(mockClient.createSession).not.toHaveBeenCalled();
    expect(mockClient.createPolicy).toHaveBeenCalledTimes(4);
  });
});
