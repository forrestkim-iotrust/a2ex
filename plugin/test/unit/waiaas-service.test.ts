import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { ChildProcess } from "node:child_process";

// ---------------------------------------------------------------------------
// Mocks — vi.mock is hoisted, so use vi.hoisted for shared refs
// ---------------------------------------------------------------------------

const { mockExecFile, mockSpawn } = vi.hoisted(() => ({
  mockExecFile: vi.fn(),
  mockSpawn: vi.fn(),
}));

vi.mock("node:child_process", () => ({
  execFile: mockExecFile,
  spawn: mockSpawn,
}));

// promisify(execFile) uses the callback signature
mockExecFile.mockImplementation(
  (
    _cmd: string,
    _args: string[],
    cb?: (err: Error | null, stdout: string, stderr: string) => void,
  ) => {
    if (cb) cb(null, "", "");
  },
);

// Import under test — after mocks are registered
import {
  startWaiaas,
  isWaiaasRunning,
  stopWaiaas,
  WaiaasStartupError,
} from "../../src/services/waiaas.service.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function createMockChild(pid: number): ChildProcess {
  return {
    pid,
    unref: vi.fn(),
  } as unknown as ChildProcess;
}

function mockFetchHealth(responses: Array<{ ok: boolean; body: unknown }>): void {
  const fetchMock = vi.fn();
  let callIndex = 0;
  fetchMock.mockImplementation(() => {
    const resp = responses[Math.min(callIndex++, responses.length - 1)]!;
    return Promise.resolve({
      ok: resp.ok,
      status: resp.ok ? 200 : 503,
      json: () => Promise.resolve(resp.body),
    });
  });
  vi.stubGlobal("fetch", fetchMock);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("waiaas.service", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    mockExecFile.mockClear();
    mockExecFile.mockImplementation(
      (
        _cmd: string,
        _args: string[],
        cb?: (err: Error | null, stdout: string, stderr: string) => void,
      ) => {
        if (cb) cb(null, "", "");
      },
    );
    mockSpawn.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // startWaiaas
  // -------------------------------------------------------------------------

  describe("startWaiaas()", () => {
    it("runs init before start, sets WAIAAS_MASTER_PASSWORD env, polls healthcheck", async () => {
      const childPid = 12345;
      mockSpawn.mockReturnValue(createMockChild(childPid));
      mockFetchHealth([{ ok: true, body: { status: "ok" } }]);

      const result = await startWaiaas({
        dataDir: "/tmp/test-data",
        masterPassword: "s3cret",
        port: 3100,
      });

      // init called first
      expect(mockExecFile).toHaveBeenCalledTimes(1);
      expect(mockExecFile.mock.calls[0]![0]).toBe("npx");
      expect(mockExecFile.mock.calls[0]![1]).toEqual([
        "-y", "@waiaas/cli",
        "init",
        "--data-dir",
        "/tmp/test-data",
      ]);

      // spawn called with correct args
      expect(mockSpawn).toHaveBeenCalledTimes(1);
      const [cmd, args, opts] = mockSpawn.mock.calls[0]!;
      expect(cmd).toBe("npx");
      expect(args).toEqual([
        "-y", "@waiaas/cli",
        "start",
        "--data-dir",
        "/tmp/test-data",
      ]);

      // masterPassword only as env var
      expect(opts.env.WAIAAS_MASTER_PASSWORD).toBe("s3cret");

      // healthcheck called
      expect(fetch).toHaveBeenCalled();
      const fetchUrl = (fetch as ReturnType<typeof vi.fn>).mock.calls[0]![0] as string;
      expect(fetchUrl).toBe("http://localhost:3100/health");

      // result
      expect(result).toEqual({ pid: childPid, port: 3100 });
    });

    it("polls healthcheck multiple times until success", async () => {
      mockSpawn.mockReturnValue(createMockChild(99));
      mockFetchHealth([
        { ok: false, body: {} },
        { ok: false, body: {} },
        { ok: true, body: { status: "ok" } },
      ]);

      const result = await startWaiaas({
        dataDir: "/tmp/d",
        masterPassword: "pw",
        port: 4000,
      });

      expect(result.pid).toBe(99);
      expect((fetch as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThanOrEqual(3);
    });

    it("throws WaiaasStartupError on healthcheck timeout", async () => {
      mockSpawn.mockReturnValue(createMockChild(55));

      // Always fail healthcheck
      vi.stubGlobal(
        "fetch",
        vi.fn().mockRejectedValue(new Error("ECONNREFUSED")),
      );

      // Capture the rejection eagerly to avoid unhandled-rejection warnings
      let caughtError: unknown;
      const promise = startWaiaas({
        dataDir: "/tmp/d",
        masterPassword: "pw",
        port: 3100,
      }).catch((err) => {
        caughtError = err;
      });

      // Advance time past the 30s timeout
      await vi.advanceTimersByTimeAsync(35_000);
      await promise;

      expect(caughtError).toBeInstanceOf(WaiaasStartupError);
      expect((caughtError as Error).message).toMatch(/healthcheck did not respond/);
    });

    it("throws if spawn returns no PID", async () => {
      mockSpawn.mockReturnValue({ pid: undefined, unref: vi.fn() });
      mockFetchHealth([{ ok: true, body: { status: "ok" } }]);

      await expect(
        startWaiaas({
          dataDir: "/tmp/d",
          masterPassword: "pw",
          port: 3100,
        }),
      ).rejects.toThrow(/no PID/);
    });
  });

  // -------------------------------------------------------------------------
  // Idempotent init
  // -------------------------------------------------------------------------

  describe("idempotent init", () => {
    it("init command is safe to re-run — uses execFile with same args", async () => {
      mockSpawn.mockReturnValue(createMockChild(1));
      mockFetchHealth([{ ok: true, body: { status: "ok" } }]);

      await startWaiaas({ dataDir: "/tmp/d", masterPassword: "pw", port: 3100 });
      await startWaiaas({ dataDir: "/tmp/d", masterPassword: "pw", port: 3100 });

      // init called twice with identical args — idempotent by design
      expect(mockExecFile).toHaveBeenCalledTimes(2);
      expect(mockExecFile.mock.calls[0]![1]).toEqual(mockExecFile.mock.calls[1]![1]);
    });
  });

  // -------------------------------------------------------------------------
  // isWaiaasRunning
  // -------------------------------------------------------------------------

  describe("isWaiaasRunning()", () => {
    it("returns true when process.kill(pid, 0) succeeds", () => {
      const spy = vi.spyOn(process, "kill").mockImplementation(() => true);
      expect(isWaiaasRunning(123)).toBe(true);
      expect(spy).toHaveBeenCalledWith(123, 0);
      spy.mockRestore();
    });

    it("returns false when process.kill(pid, 0) throws", () => {
      const spy = vi.spyOn(process, "kill").mockImplementation(() => {
        throw new Error("ESRCH");
      });
      expect(isWaiaasRunning(999)).toBe(false);
      spy.mockRestore();
    });
  });

  // -------------------------------------------------------------------------
  // stopWaiaas
  // -------------------------------------------------------------------------

  describe("stopWaiaas()", () => {
    it("sends SIGTERM to the process", () => {
      const spy = vi.spyOn(process, "kill").mockImplementation(() => true);
      stopWaiaas(123);
      expect(spy).toHaveBeenCalledWith(123, "SIGTERM");
      spy.mockRestore();
    });

    it("does not throw if process already exited", () => {
      const spy = vi.spyOn(process, "kill").mockImplementation(() => {
        throw new Error("ESRCH");
      });
      expect(() => stopWaiaas(999)).not.toThrow();
      spy.mockRestore();
    });
  });
});
