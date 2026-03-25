import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// ---------------------------------------------------------------------------
// Mock fetch globally for checkHealth tests
// ---------------------------------------------------------------------------
const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

import {
  checkHealth,
  startWaiaasHealthcheck,
} from "../../src/services/waiaas.service.js";

describe("checkHealth", () => {
  beforeEach(() => {
    mockFetch.mockReset();
  });

  it("returns true when server responds with status ok", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({ status: "ok" }),
    });

    const result = await checkHealth("http://localhost:3100");
    expect(result).toBe(true);
    expect(mockFetch).toHaveBeenCalledWith(
      "http://localhost:3100/health",
      { method: "GET" },
    );
  });

  it("returns false when server responds with non-ok status", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: async () => ({ status: "error" }),
    });
    expect(await checkHealth("http://localhost:3100")).toBe(false);
  });

  it("returns false when fetch throws (connection refused)", async () => {
    mockFetch.mockRejectedValueOnce(new Error("ECONNREFUSED"));
    expect(await checkHealth("http://localhost:3100")).toBe(false);
  });

  it("returns false when response is not ok (e.g. 500)", async () => {
    mockFetch.mockResolvedValueOnce({ ok: false });
    expect(await checkHealth("http://localhost:3100")).toBe(false);
  });
});

describe("startWaiaasHealthcheck", () => {
  let mockCheckFn: ReturnType<typeof vi.fn>;
  let mockStartFn: ReturnType<typeof vi.fn>;
  let mockStopFn: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    mockCheckFn = vi.fn();
    mockStartFn = vi.fn().mockResolvedValue({ pid: 9999, port: 3100 });
    mockStopFn = vi.fn();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function makeHandle(overrides: Record<string, any> = {}) {
    return startWaiaasHealthcheck({
      pid: 1234,
      port: 3100,
      restartOptions: { dataDir: "/tmp/test", masterPassword: "test", port: 3100 },
      intervalMs: 1000,
      _checkFn: mockCheckFn,
      _startFn: mockStartFn,
      _stopFn: mockStopFn,
      ...overrides,
    });
  }

  it("3 consecutive failures trigger restart and onRestart receives new PID", async () => {
    const onRestart = vi.fn();
    const handle = makeHandle({ onRestart });

    // 3 consecutive failures
    mockCheckFn.mockResolvedValue(false);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(mockStopFn).toHaveBeenCalledWith(1234);
    expect(mockStartFn).toHaveBeenCalledTimes(1);
    expect(onRestart).toHaveBeenCalledWith(9999);

    handle.stop();
  });

  it("successful health response resets failure count", async () => {
    const onRestart = vi.fn();
    const handle = makeHandle({ onRestart });

    // 2 failures
    mockCheckFn.mockResolvedValueOnce(false);
    await vi.advanceTimersByTimeAsync(1000);
    mockCheckFn.mockResolvedValueOnce(false);
    await vi.advanceTimersByTimeAsync(1000);

    // 1 success — resets count
    mockCheckFn.mockResolvedValueOnce(true);
    await vi.advanceTimersByTimeAsync(1000);

    // 2 more failures — still under threshold
    mockCheckFn.mockResolvedValueOnce(false);
    await vi.advanceTimersByTimeAsync(1000);
    mockCheckFn.mockResolvedValueOnce(false);
    await vi.advanceTimersByTimeAsync(1000);

    expect(onRestart).not.toHaveBeenCalled();

    handle.stop();
  });

  it("stop() prevents further healthcheck calls", async () => {
    const handle = makeHandle();

    handle.stop();

    await vi.advanceTimersByTimeAsync(5000);
    expect(mockCheckFn).not.toHaveBeenCalled();
  });

  it("isStopping prevents restart even when threshold is reached", async () => {
    const onRestart = vi.fn();
    const handle = makeHandle({ onRestart });

    // 2 failures
    mockCheckFn.mockResolvedValue(false);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);

    // Stop before 3rd failure
    handle.stop();

    await vi.advanceTimersByTimeAsync(5000);
    expect(onRestart).not.toHaveBeenCalled();
  });

  it("onRestart callback receives new PID after restart", async () => {
    const onRestart = vi.fn();
    mockStartFn.mockResolvedValue({ pid: 5555, port: 3100 });
    const handle = makeHandle({ onRestart });

    mockCheckFn.mockResolvedValue(false);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(onRestart).toHaveBeenCalledWith(5555);

    handle.stop();
  });

  it("restart failure does not crash the loop", async () => {
    const onRestart = vi.fn();
    mockStartFn.mockRejectedValueOnce(new Error("spawn failed"));
    const handle = makeHandle({ onRestart });

    mockCheckFn.mockResolvedValue(false);
    // 3 failures → restart attempt (will fail)
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(onRestart).not.toHaveBeenCalled();

    // Loop continues — 3 more failures with working startFn
    mockStartFn.mockResolvedValue({ pid: 7777, port: 3100 });
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(onRestart).toHaveBeenCalledWith(7777);

    handle.stop();
  });
});
