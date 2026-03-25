/**
 * L1.5 lifecycle test: MCP dynamic tool integration.
 *
 * Verifies the end-to-end flow:
 *   plugin-simulator → register → startServices(stateDir with running phase + binaryPath)
 *   → resolveTools → `a2ex.onboarding.bootstrap_install` tool exists and executes
 *   → mock MCP server returns expected response.
 */

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  createPluginSimulator,
  resetPluginState,
} from "./plugin-simulator.js";
import { stopA2ex } from "../../src/services/a2ex.service.js";
import {
  STATE_SUBDIR,
  STATE_FILENAME,
} from "../../src/constants.js";
import type { A2exPluginState } from "../../src/state/plugin-state.js";

const MOCK_SERVER_PATH = resolve(
  import.meta.dirname,
  "mock-mcp-server.mjs",
);

describe("MCP dynamic tools lifecycle", () => {
  let stateDir: string;

  beforeEach(async () => {
    resetPluginState();
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-mcp-lifecycle-"));
  });

  afterEach(async () => {
    // Clean up MCP subprocess before resetting state
    await stopA2ex();
    resetPluginState();
    await rm(stateDir, { recursive: true, force: true });
  });

  it("resolveTools exposes a2ex.* dynamic tools from mock MCP server", async () => {
    // Write a state file indicating running phase with a binaryPath
    // pointing to the mock MCP server
    const stateSubdir = join(stateDir, STATE_SUBDIR);
    await mkdir(stateSubdir, { recursive: true });

    const state: A2exPluginState = {
      phase: "running",
      waiaasPort: 3100,
      waiaasPid: 424242,
      binaryPath: MOCK_SERVER_PATH,
      lastUpdated: new Date().toISOString(),
    };
    await writeFile(
      join(stateSubdir, STATE_FILENAME),
      JSON.stringify(state, null, 2),
    );

    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    const tools = sim.resolveTools();

    // Should have both waiaas tools AND dynamic a2ex.* tools
    expect(tools).not.toBeNull();

    const toolNames = tools!.map((t) => t.name);

    // Dynamic a2ex.* tool must be present
    expect(toolNames).toContain("a2ex.onboarding.bootstrap_install");

    // waiaas tools should also be present
    expect(toolNames.some((n) => n.startsWith("waiaas."))).toBe(true);
  });

  it("a2ex.onboarding.bootstrap_install executes and returns mock response", async () => {
    const stateSubdir = join(stateDir, STATE_SUBDIR);
    await mkdir(stateSubdir, { recursive: true });

    const state: A2exPluginState = {
      phase: "running",
      waiaasPort: 3100,
      waiaasPid: 424242,
      binaryPath: MOCK_SERVER_PATH,
      lastUpdated: new Date().toISOString(),
    };
    await writeFile(
      join(stateSubdir, STATE_FILENAME),
      JSON.stringify(state, null, 2),
    );

    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();

    const bootstrapTool = tools!.find(
      (t) => t.name === "a2ex.onboarding.bootstrap_install",
    );
    expect(bootstrapTool).toBeDefined();

    const result = await bootstrapTool!.execute("test", {
      url: "https://example.com/a2ex-v2.0.0",
    });

    const text = typeof result === "string" ? result : JSON.stringify(result);
    expect(text).toContain("https://example.com/a2ex-v2.0.0");
  });

  it("resolveTools returns waiaas + bootstrap tools when a2ex not running (graceful degradation)", async () => {
    // State: bootstrapped but no binaryPath → no MCP connection
    const stateSubdir = join(stateDir, STATE_SUBDIR);
    await mkdir(stateSubdir, { recursive: true });

    const state: A2exPluginState = {
      phase: "bootstrapped",
      waiaasPort: 3100,
      waiaasPid: 424242,
      lastUpdated: new Date().toISOString(),
    };
    await writeFile(
      join(stateSubdir, STATE_FILENAME),
      JSON.stringify(state, null, 2),
    );

    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();

    const toolNames = tools!.map((t) => t.name);

    // Should have waiaas tools but no a2ex.* dynamic tools
    expect(toolNames.some((n) => n.startsWith("waiaas."))).toBe(true);
    expect(toolNames.some((n) => n.startsWith("a2ex.onboarding"))).toBe(false);
  });

  it("stopA2ex removes dynamic tools from resolveTools (graceful degradation)", async () => {
    const stateSubdir = join(stateDir, STATE_SUBDIR);
    await mkdir(stateSubdir, { recursive: true });

    const state: A2exPluginState = {
      phase: "running",
      waiaasPort: 3100,
      waiaasPid: 424242,
      binaryPath: MOCK_SERVER_PATH,
      lastUpdated: new Date().toISOString(),
    };
    await writeFile(
      join(stateSubdir, STATE_FILENAME),
      JSON.stringify(state, null, 2),
    );

    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // Verify a2ex tools are present
    let tools = sim.resolveTools();
    expect(tools!.some((t) => t.name.startsWith("a2ex.onboarding"))).toBe(true);

    // Stop a2ex
    await stopA2ex();

    // a2ex tools should be gone, waiaas still present
    tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    const toolNames = tools!.map((t) => t.name);
    expect(toolNames.some((n) => n.startsWith("a2ex.onboarding"))).toBe(false);
    expect(toolNames.some((n) => n.startsWith("waiaas."))).toBe(true);
  });
});
