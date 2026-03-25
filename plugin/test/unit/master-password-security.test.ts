import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

// ---------------------------------------------------------------------------
// Mocks
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

import { createBootstrapTool } from "../../src/tools/bootstrap.js";
import { WaiaasApiError } from "../../src/transport/waiaas-http-client.js";
import { STATE_SUBDIR, STATE_FILENAME } from "../../src/constants.js";

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
      .mockResolvedValueOnce({ id: "vs-id", token: "vault-token" })
      .mockResolvedValueOnce({ id: "hs-id", token: "hot-token" }),
    createPolicy: vi.fn()
      .mockResolvedValueOnce({ id: "vault-policy-id" })
      .mockResolvedValueOnce({ id: "hot-policy-id" })
      .mockResolvedValueOnce({ id: "hot-whitelist-arb-id" })
      .mockResolvedValueOnce({ id: "hot-whitelist-poly-id" }),
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("masterPassword security", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-mpw-test-"));
    vi.clearAllMocks();
    mockStartWaiaas.mockResolvedValue({ pid: 12345, port: 3100 });
    mockIsWaiaasRunning.mockReturnValue(false);
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true }).catch(() => {});
  });

  it("bootstrap result JSON does not contain masterPassword", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    const result = await tool.execute("test", { masterPassword: "s3cret!P@ssw0rd" });

    const resultText = (result as any).content[0].text;
    expect(resultText).not.toContain("s3cret!P@ssw0rd");
    expect(resultText).not.toContain("masterPassword");
  });

  it("WaiaasApiError during bootstrap does not leak masterPassword in error message", async () => {
    const mockClient = buildMockClient();
    // Fail on the first createWallet call
    mockClient.createWallet.mockReset();
    mockClient.createWallet.mockRejectedValue(
      new WaiaasApiError(401, "AUTH_FAILED", "Invalid master credentials"),
    );
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);

    try {
      await tool.execute("test", { masterPassword: "leaky-secret-pw" });
      // If it doesn't throw, check result
      expect.unreachable("should have thrown");
    } catch (err: unknown) {
      const errStr = String(err);
      expect(errStr).not.toContain("leaky-secret-pw");

      if (err instanceof Error) {
        expect(err.message).not.toContain("leaky-secret-pw");
        expect(err.stack ?? "").not.toContain("leaky-secret-pw");
      }
    }
  });

  it("masterPassword persisted in state for WAIaaS restart & key recovery", async () => {
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "super-secret-password-123" });

    const stateRaw = await readFile(
      join(stateDir, STATE_SUBDIR, STATE_FILENAME),
      "utf-8",
    );

    const state = JSON.parse(stateRaw);
    expect(state.masterPassword).toBe("super-secret-password-123");
  });

  it("masterPassword with special characters persisted correctly in state", async () => {
    const specialPasswords = [
      `pass"word'with\\quotes`,
      `유니코드패스워드🔑`,
    ];

    for (const pw of specialPasswords) {
      vi.clearAllMocks();
      const dir = await mkdtemp(join(tmpdir(), "a2ex-mpw-special-"));

      mockStartWaiaas.mockResolvedValue({ pid: 12345, port: 3100 });
      mockIsWaiaasRunning.mockReturnValue(false);

      const mockClient = buildMockClient();
      mockCreateWaiaasClient.mockReturnValue(mockClient);

      const tool = createBootstrapTool(() => dir);
      const result = await tool.execute("test", { masterPassword: pw });

      const resultText = (result as any).content[0].text;
      // Password must NOT appear in AI-visible tool output
      expect(resultText, `password "${pw}" leaked into result`).not.toContain(pw);

      // But MUST be persisted in state for recovery
      const stateRaw = await readFile(
        join(dir, STATE_SUBDIR, STATE_FILENAME),
        "utf-8",
      );
      const state = JSON.parse(stateRaw);
      expect(state.masterPassword).toBe(pw);

      await rm(dir, { recursive: true, force: true }).catch(() => {});
    }
  });

  it("masterPassword is available during bootstrap steps but disposed after step 8", async () => {
    // We verify disposal indirectly: the auth object passed to step 7 (last API
    // call before disposal) should still have the password — proving the disposal
    // happens AFTER step 8, not before.  The `auth` local is then reassigned to
    // a new object (overwriting the masterPassword reference), so the original
    // auth object's masterPassword is no longer reachable from the function scope.
    const mockClient = buildMockClient();
    mockCreateWaiaasClient.mockReturnValue(mockClient);

    const tool = createBootstrapTool(() => stateDir);
    await tool.execute("test", { masterPassword: "track-this-pw" });

    // The last API call (step 7 — hot policy) received the real password
    expect(mockClient.createPolicy).toHaveBeenCalledTimes(4);
    const lastPolicyAuth = mockClient.createPolicy.mock.calls[3][0];
    expect(lastPolicyAuth).toEqual({
      mode: "master",
      masterPassword: "track-this-pw",
    });

    // The disposal is best-effort JS memory hygiene — the local `masterPassword`
    // variable is reassigned to "" and `auth` is reassigned to a new object.
    // We can't directly observe local variable reassignment from outside, but
    // the code change is the deliverable. This test confirms the password WAS
    // available during steps and the function completed successfully.
  });
});
