import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

const { mockCreateWaiaasClient } = vi.hoisted(() => ({
  mockCreateWaiaasClient: vi.fn(),
}));

vi.mock("../../src/transport/waiaas-http-client.js", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../src/transport/waiaas-http-client.js")>();
  return {
    ...orig,
    createWaiaasClient: mockCreateWaiaasClient,
  };
});

import { createWaiaasTools } from "../../src/tools/waiaas-tools.js";
import { writeState, type A2exPluginState } from "../../src/state/plugin-state.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** State with both vault and hot session tokens — the key differentiator. */
function bootstrappedState(): A2exPluginState {
  return {
    phase: "bootstrapped",
    waiaasPort: 3100,
    waiaasPid: 11111,
    vaultWalletId: "vault-w",
    vaultAddress: "0xVAULT",
    hotWalletId: "hot-w",
    hotAddress: "0xHOT",
    vaultSessionToken: "vault-session-tok-CORRECT",
    hotSessionToken: "hot-session-tok-WRONG-FOR-WAIAAS-TOOLS",
    policyIds: { vault: "vp", hot: "hp" },
    lastUpdated: new Date().toISOString(),
  };
}

function buildMockClient() {
  return {
    health: vi.fn(),
    createWallet: vi.fn(),
    createSession: vi.fn(),
    createPolicy: vi.fn().mockResolvedValue({ id: "policy-id" }),
    getBalance: vi.fn().mockResolvedValue({ balance: "100", symbol: "ETH" }),
    getAddress: vi.fn().mockResolvedValue({ address: "0xABC" }),
    sendTransaction: vi.fn().mockResolvedValue({ id: "tx-1" }),
    signMessage: vi.fn().mockResolvedValue({ signature: "0xSIG" }),
    getTransaction: vi.fn().mockResolvedValue({ id: "tx-1", status: "confirmed" }),
    listTransactions: vi.fn().mockResolvedValue({ transactions: [] }),
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("session scope — waiaas-tools token selection", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-scope-test-"));
    vi.clearAllMocks();
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true }).catch(() => {});
  });

  it("get_balance uses vaultSessionToken (session auth), not hotSessionToken", async () => {
    await writeState(stateDir, bootstrappedState());
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tools = createWaiaasTools(() => stateDir);
    const getBalance = tools.find((t) => t.name.includes("get_balance"))!;

    await getBalance.execute("test", { walletId: "vault-w", network: "arbitrum-mainnet" });

    // The auth arg passed to the client method must be session mode with vault token
    expect(mockClient.getBalance).toHaveBeenCalledOnce();
    const [authArg] = mockClient.getBalance.mock.calls[0];
    expect(authArg).toEqual({
      mode: "session",
      token: "vault-session-tok-CORRECT",
    });
  });

  it("vault tools use vaultSessionToken while sign_message uses hotSessionToken", async () => {
    await writeState(stateDir, bootstrappedState());
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tools = createWaiaasTools(() => stateDir);

    // Call each tool and collect auth args
    const callSpecs: Array<{ tool: string; params: Record<string, unknown>; clientMethod: string }> = [
      { tool: "get_balance", params: { walletId: "w", network: "n" }, clientMethod: "getBalance" },
      { tool: "get_address", params: { walletId: "w", network: "n" }, clientMethod: "getAddress" },
      { tool: "call_contract", params: { walletId: "w", network: "n", to: "0x1", calldata: "0x" }, clientMethod: "sendTransaction" },
      { tool: "send_token", params: { walletId: "w", network: "n", to: "0x1", amount: "100" }, clientMethod: "sendTransaction" },
      { tool: "get_transaction", params: { walletId: "w", transactionId: "tx-1" }, clientMethod: "getTransaction" },
      { tool: "sign_message", params: { walletId: "w", network: "n", message: "hello" }, clientMethod: "signMessage" },
      { tool: "list_transactions", params: { walletId: "w", network: "n" }, clientMethod: "listTransactions" },
    ];

    for (const spec of callSpecs) {
      vi.clearAllMocks();
      mockCreateWaiaasClient.mockReturnValue(buildMockClient());

      const tool = tools.find((t) => t.name.includes(spec.tool))!;
      expect(tool, `tool not found: ${spec.tool}`).toBeDefined();

      await tool.execute("test", spec.params);

      const freshClient = mockCreateWaiaasClient.mock.results[0].value;
      const method = freshClient[spec.clientMethod as keyof typeof freshClient] as ReturnType<typeof vi.fn>;
      expect(method, `${spec.tool} should call client.${spec.clientMethod}`).toHaveBeenCalledOnce();

      const [authArg] = method.mock.calls[0];
      expect(authArg, `${spec.tool} auth`).toEqual(
        spec.tool === "sign_message"
          ? {
            mode: "session",
            token: "hot-session-tok-WRONG-FOR-WAIAAS-TOOLS",
          }
          : {
            mode: "session",
            token: "vault-session-tok-CORRECT",
          },
      );
    }
  });

  it("readStateAndAuth throws if vaultSessionToken is missing", async () => {
    const stateNoToken: A2exPluginState = {
      phase: "bootstrapping",
      waiaasPort: 3100,
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, stateNoToken);

    const tools = createWaiaasTools(() => stateDir);
    const getBalance = tools.find((t) => t.name.includes("get_balance"))!;

    const result = await getBalance.execute("test", { walletId: "w", network: "n" });
    const parsed = JSON.parse((result as any).content[0].text);
    expect(parsed.error).toContain("vaultSessionToken");
  });

  it("sign_message fails when hotSessionToken is missing", async () => {
    const stateNoHotToken: A2exPluginState = {
      ...bootstrappedState(),
      hotSessionToken: undefined,
    };
    await writeState(stateDir, stateNoHotToken);

    const tools = createWaiaasTools(() => stateDir);
    const signMessage = tools.find((t) => t.name.includes("sign_message"))!;

    const result = await signMessage.execute("test", {
      walletId: "w",
      network: "n",
      message: "hello",
    });
    const parsed = JSON.parse((result as any).content[0].text);
    expect(parsed.error).toContain("hotSessionToken");
  });
});
