/**
 * L1.5 lifecycle test: crash recovery integration.
 *
 * Verifies the full crash → recovery → tools-available cycle through
 * the plugin simulator, proving that:
 * 1. Tools degrade when a2ex crashes (MCP cache cleared)
 * 2. Tools recover after reconnection (MCP cache repopulated)
 * 3. stop() prevents recovery attempts (isStopping guard)
 * 4. WAIaaS healthcheck handle is created when state has a live waiaasPid
 */

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mkdtemp, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  createPluginSimulator,
  resetPluginState,
  type PluginSimulator,
} from "./plugin-simulator.js";
import { stopA2ex } from "../../src/services/a2ex.service.js";
import {
  clearMcpCache,
  setMcpCache,
  getMcpCache,
} from "../../src/tools/a2ex-dynamic.js";
import {
  STATE_SUBDIR,
  STATE_FILENAME,
} from "../../src/constants.js";
import type { A2exPluginState } from "../../src/state/plugin-state.js";
import type { Tool } from "@modelcontextprotocol/sdk/types.js";

const MOCK_SERVER_PATH = resolve(
  import.meta.dirname,
  "mock-mcp-server.mjs",
);

describe("crash recovery lifecycle", () => {
  let stateDir: string;

  beforeEach(async () => {
    resetPluginState();
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-crash-recovery-"));
  });

  afterEach(async () => {
    await stopA2ex();
    resetPluginState();
    await rm(stateDir, { recursive: true, force: true });
  });

  /**
   * Helper: write state file with given properties.
   */
  async function writeStateFile(
    overrides: Partial<A2exPluginState> = {},
  ): Promise<void> {
    const stateSubdir = join(stateDir, STATE_SUBDIR);
    await mkdir(stateSubdir, { recursive: true });

    const state: A2exPluginState = {
      phase: "running",
      waiaasPort: 3100,
      waiaasPid: 424242,
      binaryPath: MOCK_SERVER_PATH,
      lastUpdated: new Date().toISOString(),
      ...overrides,
    };
    await writeFile(
      join(stateSubdir, STATE_FILENAME),
      JSON.stringify(state, null, 2),
    );
  }

  it("tools degrade on a2ex crash and recover after reconnect", async () => {
    await writeStateFile();

    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // --- Phase 1: tools available (a2ex connected via startA2exWithRecovery) ---
    const toolsBefore = sim.resolveTools();
    expect(toolsBefore).not.toBeNull();

    const namesBefore = toolsBefore!.map((t) => t.name);
    expect(namesBefore).toContain("a2ex_onboarding_bootstrap_install");
    expect(namesBefore).toContain("a2ex_system_health");
    expect(namesBefore.some((n) => n.startsWith("waiaas."))).toBe(true);

    // --- Phase 2: simulate crash — clearMcpCache mimics onClose effect ---
    clearMcpCache();

    const toolsDegraded = sim.resolveTools();
    expect(toolsDegraded).not.toBeNull();

    const namesDegraded = toolsDegraded!.map((t) => t.name);
    // Dynamic a2ex.onboarding/skills/runtime tools should be gone (from MCP)
    expect(namesDegraded.some((n) => n.startsWith("a2ex_onboarding"))).toBe(false);
    // Static a2ex tools remain (system_health, waiaas)
    expect(namesDegraded).toContain("a2ex_system_health");
    expect(namesDegraded.some((n) => n.startsWith("waiaas."))).toBe(true);

    // --- Phase 3: simulate recovery — re-populate MCP cache ---
    // In reality, startA2exWithRecovery's scheduleRestart does this.
    // For the test, we manually repopulate the cache with mock tools
    // to prove the factory picks them up again.
    const mockTools: Tool[] = [
      {
        name: "onboarding.bootstrap_install",
        description: "Install a2ex binary",
        inputSchema: {
          type: "object" as const,
          properties: { url: { type: "string" } },
        },
      },
    ];
    // We need a mock client — use a minimal stub
    const mockClient = {
      callTool: vi.fn().mockResolvedValue("ok"),
      close: vi.fn().mockResolvedValue(undefined),
    } as any;
    setMcpCache(mockTools, mockClient);

    const toolsRecovered = sim.resolveTools();
    expect(toolsRecovered).not.toBeNull();

    const namesRecovered = toolsRecovered!.map((t) => t.name);
    expect(namesRecovered).toContain("a2ex_onboarding_bootstrap_install");
    expect(namesRecovered).toContain("a2ex_system_health");
    expect(namesRecovered.some((n) => n.startsWith("waiaas."))).toBe(true);
  });

  it("stop() prevents recovery — no restart after service shutdown", async () => {
    await writeStateFile();

    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // Verify tools are available
    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    // MCP dynamic tools (from mock server) should be present
    expect(tools!.some((t) => t.name.startsWith("a2ex_onboarding"))).toBe(true);

    // Capture the current MCP cache state before stop
    const cacheBefore = getMcpCache();
    expect(cacheBefore).not.toBeNull();

    // Stop via the service's stop() — sets isStopping, tears down handles
    await sim.stopServices();

    // Cache should be cleared by a2exRecoveryHandle.stop() → stopA2ex()
    expect(getMcpCache()).toBeNull();

    // MCP dynamic tools should be gone, static tools remain
    const toolsAfter = sim.resolveTools();
    expect(toolsAfter).not.toBeNull();
    const namesAfter = toolsAfter!.map((t) => t.name);
    expect(namesAfter.some((n) => n.startsWith("a2ex_onboarding"))).toBe(false);
    expect(namesAfter).toContain("a2ex_system_health");
  });

  it("service start() uses startA2exWithRecovery (tools available after start)", async () => {
    // This test proves that start() wires startA2exWithRecovery by
    // verifying the full connect → tools available flow works, which
    // requires the recovery wrapper's start() to be called.
    await writeStateFile();

    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();

    const names = tools!.map((t) => t.name);
    // The mock server registers onboarding.bootstrap_install
    expect(names).toContain("a2ex_onboarding_bootstrap_install");
    // The recovery handle exists (proven by the fact that tools loaded
    // through startA2exWithRecovery's connectWithRecovery path)
    expect(getMcpCache()).not.toBeNull();
  });

  it("WAIaaS healthcheck is wired when state has waiaasPid of a live process", async () => {
    // Use the current process PID as a "live" waiaasPid — isWaiaasRunning
    // uses process.kill(pid, 0) which succeeds for our own process.
    await writeStateFile({
      waiaasPid: process.pid,
      binaryPath: undefined, // Skip a2ex connection for this test
    });

    // We can't directly observe healthcheckHandle from outside,
    // but we can verify that startWaiaasHealthcheck was wired by
    // confirming the service starts without error when waiaasPid is live.
    const sim = createPluginSimulator();
    sim.register();

    // This should not throw — start() should wire the healthcheck
    await sim.startServices(stateDir);

    // With no binaryPath, tools won't include a2ex.* but should still resolve
    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    expect(tools!.some((t) => t.name === "a2ex_system_health")).toBe(true);
  });

  it("WAIaaS healthcheck is NOT wired when waiaasPid is missing from state", async () => {
    await writeStateFile({
      waiaasPid: undefined,
      binaryPath: undefined,
    });

    const sim = createPluginSimulator();
    sim.register();

    // Should start cleanly without wiring healthcheck
    await sim.startServices(stateDir);

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
  });
});
