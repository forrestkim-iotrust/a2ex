import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  STATE_SUBDIR,
  STATE_FILENAME,
  TOOL_WAIAAS_GET_BALANCE,
  TOOL_WAIAAS_GET_ADDRESS,
  TOOL_WAIAAS_CALL_CONTRACT,
  TOOL_WAIAAS_SEND_TOKEN,
  TOOL_WAIAAS_GET_TRANSACTION,
  TOOL_WAIAAS_SIGN_MESSAGE,
  TOOL_WAIAAS_LIST_TRANSACTIONS,
} from "../../src/constants.js";
import { createWaiaasTools } from "../../src/tools/waiaas-tools.js";
import type { AnyAgentTool } from "../../src/types/openclaw-plugin.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const BOOTSTRAPPED_STATE = {
  phase: "bootstrapped",
  waiaasPort: 3100,
  vaultSessionToken: "sess-token-abc",
  hotSessionToken: "hot-token-xyz",
  vaultWalletId: "vault-w1",
  hotWalletId: "hot-w1",
  lastUpdated: new Date().toISOString(),
};

function mockFetchOk(body: Record<string, unknown>) {
  return vi.fn().mockResolvedValue({
    ok: true,
    json: async () => body,
  } as unknown as Response);
}

function mockFetchError(status: number, body: Record<string, unknown>) {
  return vi.fn().mockResolvedValue({
    ok: false,
    status,
    statusText: "Bad Request",
    json: async () => body,
  } as unknown as Response);
}

function parseResult(result: unknown): { content: string; isError?: boolean } {
  const r = result as { content: { type: string; text: string }[]; isError?: boolean };
  return { content: r.content[0].text, isError: r.isError };
}

async function writeState(stateDir: string, state: Record<string, unknown>) {
  const dir = join(stateDir, STATE_SUBDIR);
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, STATE_FILENAME), JSON.stringify(state), "utf-8");
}

// ---------------------------------------------------------------------------
// Test suite
// ---------------------------------------------------------------------------

describe("createWaiaasTools", () => {
  let stateDir: string;
  let tools: AnyAgentTool[];
  const originalFetch = globalThis.fetch;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "waiaas-tools-"));
    await writeState(stateDir, BOOTSTRAPPED_STATE);
    tools = createWaiaasTools(() => stateDir);
  });

  afterEach(async () => {
    globalThis.fetch = originalFetch;
    await rm(stateDir, { recursive: true, force: true });
  });

  it("returns exactly 7 tools with correct names", () => {
    expect(tools).toHaveLength(7);
    const names = tools.map((t) => t.name);
    expect(names).toEqual([
      TOOL_WAIAAS_GET_BALANCE,
      TOOL_WAIAAS_GET_ADDRESS,
      TOOL_WAIAAS_CALL_CONTRACT,
      TOOL_WAIAAS_SEND_TOKEN,
      TOOL_WAIAAS_GET_TRANSACTION,
      TOOL_WAIAAS_SIGN_MESSAGE,
      TOOL_WAIAAS_LIST_TRANSACTIONS,
    ]);
  });

  it("all tools require walletId in parameters schema", () => {
    for (const tool of tools) {
      const schema = tool.parameters as { required?: string[] };
      expect(schema.required, `${tool.name} should require walletId`).toContain("walletId");
    }
  });

  // -------------------------------------------------------------------------
  // Shared error cases
  // -------------------------------------------------------------------------

  describe("shared error handling", () => {
    it("returns error when stateDir is null", async () => {
      const nullTools = createWaiaasTools(() => null);
      for (const tool of nullTools) {
        const result = parseResult(await tool.execute("test", { walletId: "w1", network: "eth" }));
        expect(result.isError).toBe(true);
        expect(result.content).toContain("Not bootstrapped");
      }
    });

    it("returns error when vaultSessionToken is missing", async () => {
      const noTokenState = { ...BOOTSTRAPPED_STATE, vaultSessionToken: undefined };
      await writeState(stateDir, noTokenState);

      const tool = tools[0]; // get_balance
      const result = parseResult(await tool.execute("test", { walletId: "w1", network: "eth" }));
      expect(result.isError).toBe(true);
      expect(result.content).toContain("vaultSessionToken not found");
    });

    it("sign_message returns error when hotSessionToken is missing", async () => {
      const noTokenState = { ...BOOTSTRAPPED_STATE, hotSessionToken: undefined };
      await writeState(stateDir, noTokenState);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_SIGN_MESSAGE)!;
      const result = parseResult(await tool.execute("test", {
        walletId: "hot-w1",
        network: "eth",
        message: "hello",
      }));
      expect(result.isError).toBe(true);
      expect(result.content).toContain("hotSessionToken not found");
    });

    it("propagates WaiaasApiError with error code", async () => {
      globalThis.fetch = mockFetchError(403, {
        errorCode: "SESSION_EXPIRED",
        errorMessage: "Token expired",
      });

      const tool = tools[0]; // get_balance
      const result = parseResult(await tool.execute("test", { walletId: "w1", network: "eth" }));
      expect(result.isError).toBe(true);
      expect(result.content).toContain("SESSION_EXPIRED");
      expect(result.content).toContain("Token expired");
    });
  });

  // -------------------------------------------------------------------------
  // get_balance
  // -------------------------------------------------------------------------

  describe("get_balance", () => {
    it("happy path — returns balance in MCP envelope", async () => {
      const body = { balance: "1000000", network: "arbitrum-mainnet" };
      globalThis.fetch = mockFetchOk(body);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_GET_BALANCE)!;
      const result = parseResult(
        await tool.execute("test", { walletId: "vault-w1", network: "arbitrum-mainnet" }),
      );

      expect(result.isError).toBeUndefined();
      expect(JSON.parse(result.content)).toEqual(body);

      // Verify fetch was called with correct URL and auth
      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      expect(call[0]).toContain("/v1/wallet/balance?");
      expect(call[0]).toContain("walletId=vault-w1");
      expect(call[0]).toContain("network=arbitrum-mainnet");
      expect(call[1].headers["Authorization"]).toBe("Bearer sess-token-abc");
    });
  });

  // -------------------------------------------------------------------------
  // get_address
  // -------------------------------------------------------------------------

  describe("get_address", () => {
    it("happy path — returns address in MCP envelope", async () => {
      const body = { address: "0xabc123", network: "arbitrum-mainnet" };
      globalThis.fetch = mockFetchOk(body);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_GET_ADDRESS)!;
      const result = parseResult(
        await tool.execute("test", { walletId: "vault-w1", network: "arbitrum-mainnet" }),
      );

      expect(result.isError).toBeUndefined();
      expect(JSON.parse(result.content)).toEqual(body);

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      expect(call[0]).toContain("/v1/wallet/address?");
      expect(call[0]).toContain("walletId=vault-w1");
    });
  });

  // -------------------------------------------------------------------------
  // call_contract
  // -------------------------------------------------------------------------

  describe("call_contract", () => {
    it("happy path — sends CONTRACT_CALL transaction", async () => {
      const body = { transactionId: "tx-1", hash: "0xfeed" };
      globalThis.fetch = mockFetchOk(body);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_CALL_CONTRACT)!;
      const result = parseResult(
        await tool.execute("test", {
          walletId: "vault-w1",
          network: "arbitrum-mainnet",
          to: "0xcontract",
          calldata: "0xdeadbeef",
          value: "100",
        }),
      );

      expect(result.isError).toBeUndefined();
      expect(JSON.parse(result.content)).toEqual(body);

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      expect(call[0]).toContain("/v1/transactions/send");
      const sentBody = JSON.parse(call[1].body);
      expect(sentBody.type).toBe("CONTRACT_CALL");
      expect(sentBody.to).toBe("0xcontract");
      expect(sentBody.calldata).toBe("0xdeadbeef");
      expect(sentBody.value).toBe("100");
    });
  });

  // -------------------------------------------------------------------------
  // send_token
  // -------------------------------------------------------------------------

  describe("send_token", () => {
    it("happy path — sends TRANSFER transaction", async () => {
      const body = { transactionId: "tx-2" };
      globalThis.fetch = mockFetchOk(body);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_SEND_TOKEN)!;
      const result = parseResult(
        await tool.execute("test", {
          walletId: "hot-w1",
          network: "arbitrum-mainnet",
          to: "0xrecipient",
          amount: "5000",
        }),
      );

      expect(result.isError).toBeUndefined();
      expect(JSON.parse(result.content)).toEqual(body);

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      const sentBody = JSON.parse(call[1].body);
      expect(sentBody.type).toBe("TRANSFER");
      expect(sentBody.value).toBe("5000");
      expect(sentBody.to).toBe("0xrecipient");
    });
  });

  // -------------------------------------------------------------------------
  // get_transaction
  // -------------------------------------------------------------------------

  describe("get_transaction", () => {
    it("happy path — interpolates transactionId into path", async () => {
      const body = { id: "tx-42", status: "confirmed", hash: "0xabc" };
      globalThis.fetch = mockFetchOk(body);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_GET_TRANSACTION)!;
      const result = parseResult(
        await tool.execute("test", { walletId: "vault-w1", transactionId: "tx-42" }),
      );

      expect(result.isError).toBeUndefined();
      expect(JSON.parse(result.content)).toEqual(body);

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      expect(call[0]).toContain("/v1/transactions/tx-42");
    });
  });

  // -------------------------------------------------------------------------
  // sign_message
  // -------------------------------------------------------------------------

  describe("sign_message", () => {
    it("happy path — signs message", async () => {
      const body = { signature: "0xsig123" };
      globalThis.fetch = mockFetchOk(body);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_SIGN_MESSAGE)!;
      const result = parseResult(
        await tool.execute("test", {
          walletId: "vault-w1",
          network: "arbitrum-mainnet",
          message: "hello world",
        }),
      );

      expect(result.isError).toBeUndefined();
      expect(JSON.parse(result.content)).toEqual(body);

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      expect(call[0]).toContain("/v1/transactions/sign-message");
      const sentBody = JSON.parse(call[1].body);
      expect(sentBody.message).toBe("hello world");
      expect(sentBody.walletId).toBe("vault-w1");
    });

    it("passes optional signType and typedData", async () => {
      globalThis.fetch = mockFetchOk({ signature: "0xsig456" });

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_SIGN_MESSAGE)!;
      await tool.execute("test", {
        walletId: "vault-w1",
        network: "arbitrum-mainnet",
        message: "typed",
        signType: "EIP-712",
        typedData: '{"types":{}}',
      });

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      const sentBody = JSON.parse(call[1].body);
      expect(sentBody.signType).toBe("EIP-712");
      expect(sentBody.typedData).toBe('{"types":{}}');
    });
  });

  // -------------------------------------------------------------------------
  // list_transactions
  // -------------------------------------------------------------------------

  describe("list_transactions", () => {
    it("happy path — lists transactions", async () => {
      const body = { transactions: [{ id: "tx-1", status: "confirmed" }] };
      globalThis.fetch = mockFetchOk(body);

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_LIST_TRANSACTIONS)!;
      const result = parseResult(
        await tool.execute("test", { walletId: "vault-w1", network: "arbitrum-mainnet" }),
      );

      expect(result.isError).toBeUndefined();
      expect(JSON.parse(result.content)).toEqual(body);

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      expect(call[0]).toContain("/v1/transactions?");
      expect(call[0]).toContain("walletId=vault-w1");
    });

    it("passes optional status and limit params", async () => {
      globalThis.fetch = mockFetchOk({ transactions: [] });

      const tool = tools.find((t) => t.name === TOOL_WAIAAS_LIST_TRANSACTIONS)!;
      await tool.execute("test", {
        walletId: "vault-w1",
        network: "arbitrum-mainnet",
        status: "pending",
        limit: 5,
      });

      const call = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      expect(call[0]).toContain("status=pending");
      expect(call[0]).toContain("limit=5");
    });
  });
});
