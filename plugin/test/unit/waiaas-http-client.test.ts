import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  createWaiaasClient,
  WaiaasApiError,
  type WaiaasAuth,
} from "../../src/transport/waiaas-http-client.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mockFetchOk(body: unknown): void {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: () => Promise.resolve(body),
    }),
  );
}

function mockFetchError(status: number, body: unknown): void {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status,
      statusText: "Bad Request",
      json: () => Promise.resolve(body),
    }),
  );
}

const BASE = "http://localhost:3100";
const masterAuth: WaiaasAuth = { mode: "master", masterPassword: "test-pw" };
const sessionAuth: WaiaasAuth = { mode: "session", token: "wai_sess_abc" };

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("waiaas-http-client", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  // ----- health -----
  describe("health()", () => {
    it("returns status and version on success", async () => {
      mockFetchOk({ status: "ok", version: "1.2.3" });
      const client = createWaiaasClient(BASE);
      const res = await client.health();
      expect(res).toEqual({ status: "ok", version: "1.2.3" });

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      expect(fetchCall[0]).toBe(`${BASE}/health`);
      expect((fetchCall[1] as RequestInit).method).toBe("GET");
    });

    it("throws WaiaasApiError on failure", async () => {
      mockFetchError(503, { error: "SERVICE_UNAVAILABLE", message: "not ready" });
      const client = createWaiaasClient(BASE);
      await expect(client.health()).rejects.toThrow(WaiaasApiError);
      try {
        await client.health();
      } catch (e) {
        const err = e as WaiaasApiError;
        expect(err.statusCode).toBe(503);
        expect(err.errorCode).toBe("SERVICE_UNAVAILABLE");
        expect(err.errorMessage).toBe("not ready");
      }
    });
  });

  // ----- createWallet -----
  describe("createWallet()", () => {
    it("sends X-Master-Password header and correct body", async () => {
      mockFetchOk({ id: "w-1", publicKey: "0xABC" });
      const client = createWaiaasClient(BASE);
      const res = await client.createWallet(masterAuth, {
        name: "a2ex-vault",
        chain: "ethereum",
        environment: "mainnet",
      });

      expect(res).toEqual({ id: "w-1", publicKey: "0xABC" });

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      expect(fetchCall[0]).toBe(`${BASE}/v1/wallets`);
      const init = fetchCall[1] as RequestInit;
      expect((init.headers as Record<string, string>)["X-Master-Password"]).toBe("test-pw");
      expect(JSON.parse(init.body as string)).toEqual({
        name: "a2ex-vault",
        chain: "ethereum",
        environment: "mainnet",
      });
    });
  });

  // ----- createSession -----
  describe("createSession()", () => {
    it("sends walletIds array in body with masterAuth", async () => {
      mockFetchOk({ id: "s-1", token: "wai_sess_xyz" });
      const client = createWaiaasClient(BASE);
      const res = await client.createSession(masterAuth, {
        walletIds: ["w-vault", "w-hot"],
      });

      expect(res).toEqual({ id: "s-1", token: "wai_sess_xyz" });

      const init = vi.mocked(fetch).mock.calls[0][1] as RequestInit;
      expect(JSON.parse(init.body as string)).toEqual({
        walletIds: ["w-vault", "w-hot"],
      });
      expect((init.headers as Record<string, string>)["X-Master-Password"]).toBe("test-pw");
    });
  });

  // ----- createPolicy -----
  describe("createPolicy()", () => {
    it("sends SPENDING_LIMIT type and rules with masterAuth", async () => {
      mockFetchOk({ id: "p-1" });
      const client = createWaiaasClient(BASE);
      const res = await client.createPolicy(masterAuth, {
        walletId: "w-vault",
        type: "SPENDING_LIMIT",
        rules: { instant_max_usd: 0, delay_max_usd: 10 },
        priority: 1,
        enabled: true,
      });

      expect(res).toEqual({ id: "p-1" });

      const init = vi.mocked(fetch).mock.calls[0][1] as RequestInit;
      const body = JSON.parse(init.body as string);
      expect(body.type).toBe("SPENDING_LIMIT");
      expect(body.rules).toEqual({ instant_max_usd: 0, delay_max_usd: 10 });
      expect(body.walletId).toBe("w-vault");
    });
  });

  // ----- sessionAuth -----
  describe("sessionAuth mode", () => {
    it("sends Authorization: Bearer header", async () => {
      mockFetchOk({ id: "w-2", publicKey: "0xDEF" });
      const client = createWaiaasClient(BASE);
      await client.createWallet(sessionAuth, {
        name: "test",
        chain: "ethereum",
        environment: "mainnet",
      });

      const init = vi.mocked(fetch).mock.calls[0][1] as RequestInit;
      const headers = init.headers as Record<string, string>;
      expect(headers["Authorization"]).toBe("Bearer wai_sess_abc");
      expect(headers["X-Master-Password"]).toBeUndefined();
    });
  });

  // ----- error handling -----
  describe("error handling", () => {
    it("preserves errorCode and errorMessage from WAIaaS JSON error body", async () => {
      mockFetchError(400, {
        errorCode: "WALLET_NAME_TAKEN",
        errorMessage: "A wallet with that name already exists",
      });
      const client = createWaiaasClient(BASE);

      await expect(
        client.createWallet(masterAuth, {
          name: "dup",
          chain: "ethereum",
          environment: "mainnet",
        }),
      ).rejects.toThrow(WaiaasApiError);

      try {
        await client.createWallet(masterAuth, {
          name: "dup",
          chain: "ethereum",
          environment: "mainnet",
        });
      } catch (e) {
        const err = e as WaiaasApiError;
        expect(err.statusCode).toBe(400);
        expect(err.errorCode).toBe("WALLET_NAME_TAKEN");
        expect(err.errorMessage).toBe("A wallet with that name already exists");
        expect(err.message).toContain("WALLET_NAME_TAKEN");
      }
    });

    it("handles non-JSON error responses gracefully", async () => {
      vi.stubGlobal(
        "fetch",
        vi.fn().mockResolvedValue({
          ok: false,
          status: 502,
          statusText: "Bad Gateway",
          json: () => Promise.reject(new Error("not json")),
        }),
      );
      const client = createWaiaasClient(BASE);

      try {
        await client.health();
      } catch (e) {
        const err = e as WaiaasApiError;
        expect(err.statusCode).toBe(502);
        expect(err.errorCode).toBe("UNKNOWN");
        expect(err.errorMessage).toBe("Bad Gateway");
      }
    });
  });

  // ----- getBalance -----
  describe("getBalance()", () => {
    it("sends GET with query params and Bearer auth", async () => {
      mockFetchOk({ balance: "1.5", network: "arbitrum-mainnet" });
      const client = createWaiaasClient(BASE);
      const res = await client.getBalance(sessionAuth, {
        walletId: "w-vault",
        network: "arbitrum-mainnet",
      });

      expect(res).toEqual({ balance: "1.5", network: "arbitrum-mainnet" });

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      const url = fetchCall[0] as string;
      expect(url).toContain(`${BASE}/v1/wallet/balance?`);
      expect(url).toContain("walletId=w-vault");
      expect(url).toContain("network=arbitrum-mainnet");
      const init = fetchCall[1] as RequestInit;
      expect(init.method).toBe("GET");
      expect((init.headers as Record<string, string>)["Authorization"]).toBe("Bearer wai_sess_abc");
    });

    it("throws WaiaasApiError on failure", async () => {
      mockFetchError(400, { errorCode: "WALLET_ID_REQUIRED", errorMessage: "walletId is required" });
      const client = createWaiaasClient(BASE);
      await expect(
        client.getBalance(sessionAuth, { walletId: "", network: "arbitrum-mainnet" }),
      ).rejects.toThrow(WaiaasApiError);
    });
  });

  // ----- getAddress -----
  describe("getAddress()", () => {
    it("sends GET with query params and Bearer auth", async () => {
      mockFetchOk({ address: "0xABC123", network: "ethereum-mainnet" });
      const client = createWaiaasClient(BASE);
      const res = await client.getAddress(sessionAuth, {
        walletId: "w-vault",
        network: "ethereum-mainnet",
      });

      expect(res).toEqual({ address: "0xABC123", network: "ethereum-mainnet" });

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      const url = fetchCall[0] as string;
      expect(url).toContain(`${BASE}/v1/wallet/address?`);
      expect(url).toContain("walletId=w-vault");
      expect(url).toContain("network=ethereum-mainnet");
      const init = fetchCall[1] as RequestInit;
      expect(init.method).toBe("GET");
      expect((init.headers as Record<string, string>)["Authorization"]).toBe("Bearer wai_sess_abc");
    });

    it("throws WaiaasApiError on failure", async () => {
      mockFetchError(404, { errorCode: "WALLET_NOT_FOUND", errorMessage: "wallet not found" });
      const client = createWaiaasClient(BASE);
      await expect(
        client.getAddress(sessionAuth, { walletId: "bad-id", network: "ethereum-mainnet" }),
      ).rejects.toThrow(WaiaasApiError);
    });
  });

  // ----- sendTransaction -----
  describe("sendTransaction()", () => {
    it("sends POST with JSON body and Bearer auth", async () => {
      mockFetchOk({ transactionId: "tx-1", hash: "0xDEAD" });
      const client = createWaiaasClient(BASE);
      const res = await client.sendTransaction(sessionAuth, {
        walletId: "w-vault",
        network: "arbitrum-mainnet",
        to: "0xRECIPIENT",
        value: "1000000",
        data: "0x",
      });

      expect(res).toEqual({ transactionId: "tx-1", hash: "0xDEAD" });

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      expect(fetchCall[0]).toBe(`${BASE}/v1/transactions/send`);
      const init = fetchCall[1] as RequestInit;
      expect(init.method).toBe("POST");
      expect((init.headers as Record<string, string>)["Authorization"]).toBe("Bearer wai_sess_abc");
      const body = JSON.parse(init.body as string);
      expect(body.walletId).toBe("w-vault");
      expect(body.to).toBe("0xRECIPIENT");
      expect(body.value).toBe("1000000");
    });

    it("throws WaiaasApiError on failure", async () => {
      mockFetchError(403, { errorCode: "POLICY_VIOLATION", errorMessage: "spending limit exceeded" });
      const client = createWaiaasClient(BASE);
      await expect(
        client.sendTransaction(sessionAuth, {
          walletId: "w-vault",
          network: "arbitrum-mainnet",
          to: "0xRECIPIENT",
          value: "999999999",
        }),
      ).rejects.toThrow(WaiaasApiError);
    });
  });

  // ----- signMessage -----
  describe("signMessage()", () => {
    it("sends POST with JSON body and Bearer auth", async () => {
      mockFetchOk({ signature: "0xSIG123" });
      const client = createWaiaasClient(BASE);
      const res = await client.signMessage(sessionAuth, {
        walletId: "w-vault",
        network: "ethereum-mainnet",
        message: "Hello World",
      });

      expect(res).toEqual({ signature: "0xSIG123" });

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      expect(fetchCall[0]).toBe(`${BASE}/v1/transactions/sign-message`);
      const init = fetchCall[1] as RequestInit;
      expect(init.method).toBe("POST");
      expect((init.headers as Record<string, string>)["Authorization"]).toBe("Bearer wai_sess_abc");
      const body = JSON.parse(init.body as string);
      expect(body.walletId).toBe("w-vault");
      expect(body.message).toBe("Hello World");
    });

    it("throws WaiaasApiError on failure", async () => {
      mockFetchError(400, { errorCode: "INVALID_MESSAGE", errorMessage: "message is required" });
      const client = createWaiaasClient(BASE);
      await expect(
        client.signMessage(sessionAuth, {
          walletId: "w-vault",
          network: "ethereum-mainnet",
          message: "",
        }),
      ).rejects.toThrow(WaiaasApiError);
    });
  });

  // ----- getTransaction -----
  describe("getTransaction()", () => {
    it("sends GET with transaction ID in path and Bearer auth", async () => {
      mockFetchOk({ id: "tx-42", status: "confirmed", hash: "0xBEEF" });
      const client = createWaiaasClient(BASE);
      const res = await client.getTransaction(sessionAuth, "tx-42");

      expect(res).toEqual({ id: "tx-42", status: "confirmed", hash: "0xBEEF" });

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      expect(fetchCall[0]).toBe(`${BASE}/v1/transactions/tx-42`);
      const init = fetchCall[1] as RequestInit;
      expect(init.method).toBe("GET");
      expect((init.headers as Record<string, string>)["Authorization"]).toBe("Bearer wai_sess_abc");
    });

    it("throws WaiaasApiError on failure", async () => {
      mockFetchError(404, { errorCode: "TRANSACTION_NOT_FOUND", errorMessage: "transaction not found" });
      const client = createWaiaasClient(BASE);
      await expect(
        client.getTransaction(sessionAuth, "bad-tx-id"),
      ).rejects.toThrow(WaiaasApiError);
    });
  });

  // ----- listTransactions -----
  describe("listTransactions()", () => {
    it("sends GET with query params including optional status and limit", async () => {
      const txList = { transactions: [{ id: "tx-1", status: "confirmed" }] };
      mockFetchOk(txList);
      const client = createWaiaasClient(BASE);
      const res = await client.listTransactions(sessionAuth, {
        walletId: "w-vault",
        network: "arbitrum-mainnet",
        status: "confirmed",
        limit: 10,
      });

      expect(res).toEqual(txList);

      const fetchCall = vi.mocked(fetch).mock.calls[0];
      const url = fetchCall[0] as string;
      expect(url).toContain(`${BASE}/v1/transactions?`);
      expect(url).toContain("walletId=w-vault");
      expect(url).toContain("network=arbitrum-mainnet");
      expect(url).toContain("status=confirmed");
      expect(url).toContain("limit=10");
      const init = fetchCall[1] as RequestInit;
      expect(init.method).toBe("GET");
      expect((init.headers as Record<string, string>)["Authorization"]).toBe("Bearer wai_sess_abc");
    });

    it("omits optional params when not provided", async () => {
      mockFetchOk({ transactions: [] });
      const client = createWaiaasClient(BASE);
      await client.listTransactions(sessionAuth, {
        walletId: "w-vault",
        network: "arbitrum-mainnet",
      });

      const url = vi.mocked(fetch).mock.calls[0][0] as string;
      expect(url).not.toContain("status=");
      expect(url).not.toContain("limit=");
    });

    it("throws WaiaasApiError on failure", async () => {
      mockFetchError(500, { errorCode: "INTERNAL_ERROR", errorMessage: "server error" });
      const client = createWaiaasClient(BASE);
      await expect(
        client.listTransactions(sessionAuth, {
          walletId: "w-vault",
          network: "arbitrum-mainnet",
        }),
      ).rejects.toThrow(WaiaasApiError);
    });
  });
});
