import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createPluginSimulator, resetPluginState } from "./plugin-simulator.js";
import { writeState } from "../../src/state/plugin-state.js";
import type { A2exPluginState } from "../../src/state/plugin-state.js";
import { TOOL_SYSTEM_HEALTH } from "../../src/constants.js";

describe("plugin lifecycle: register → start → factory per-turn resolve", () => {
  let stateDir: string;

  beforeEach(async () => {
    resetPluginState();
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-lifecycle-"));
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true });
  });

  it("factory returns bootstrap + health before service start (stateDir not captured)", () => {
    const sim = createPluginSimulator();
    sim.register();

    const tools = sim.resolveTools();
    expect(Array.isArray(tools)).toBe(true);
    expect(tools!.length).toBe(2);
  });

  it("factory returns system_health after service start with no prior state", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    expect(tools!.length).toBeGreaterThan(0);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);

    // Execute system_health — should show not_initialized (no state file)
    const healthTool = tools!.find((t) => t.name === TOOL_SYSTEM_HEALTH)!;
    const result = (await healthTool.execute("test", {})) as {
      content: Array<{ type: string; text: string }>;
    };
    const parsed = JSON.parse(result.content[0].text);
    expect(parsed.status).toBe("not_initialized");
  });

  it("factory reflects bootstrapped state on next turn after state write", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // Turn 1: no state file → not_initialized
    const turn1 = sim.resolveTools();
    expect(turn1).not.toBeNull();

    // Simulate bootstrap completing — write bootstrapped state
    const bootstrappedState: A2exPluginState = {
      phase: "bootstrapped",
      waiaasPort: 3100,
      waiaasPid: 12345,
      lastUpdated: "",
    };
    await writeState(stateDir, bootstrappedState);

    // Turn 2: state file exists with "bootstrapped" → tools reflect new phase
    const turn2 = sim.resolveTools();
    expect(turn2).not.toBeNull();

    const healthTool = turn2!.find((t) => t.name === TOOL_SYSTEM_HEALTH)!;
    const result = (await healthTool.execute("test", {})) as {
      content: Array<{ type: string; text: string }>;
    };
    const parsed = JSON.parse(result.content[0].text);
    expect(parsed.status).toBe("bootstrapped");
    expect(parsed.waiaas.pid).toBe(12345);
  });

  it("state change between turns produces different tool output", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // Turn 1: no state → not_initialized
    const turn1 = sim.resolveTools()!;
    const health1 = turn1.find((t) => t.name === TOOL_SYSTEM_HEALTH)!;
    const r1 = (await health1.execute("test", {})) as {
      content: Array<{ type: string; text: string }>;
    };
    expect(JSON.parse(r1.content[0].text).status).toBe("not_initialized");

    // Write bootstrapped state
    await writeState(stateDir, {
      phase: "bootstrapped",
      waiaasPort: 3100,
      waiaasPid: 424242,
      lastUpdated: "",
    });

    // Turn 2: bootstrapped
    const turn2 = sim.resolveTools()!;
    const health2 = turn2.find((t) => t.name === TOOL_SYSTEM_HEALTH)!;
    const r2 = (await health2.execute("test", {})) as {
      content: Array<{ type: string; text: string }>;
    };
    expect(JSON.parse(r2.content[0].text).status).toBe("bootstrapped");

    // Write running state
    await writeState(stateDir, {
      phase: "running",
      waiaasPort: 3100,
      waiaasPid: 999,
      lastUpdated: "",
    });

    // Turn 3: running — different output from turn 2
    const turn3 = sim.resolveTools()!;
    const health3 = turn3.find((t) => t.name === TOOL_SYSTEM_HEALTH)!;
    const r3 = (await health3.execute("test", {})) as {
      content: Array<{ type: string; text: string }>;
    };
    const parsed3 = JSON.parse(r3.content[0].text);
    expect(parsed3.status).toBe("running");
    expect(parsed3.a2ex.connected).toBe(false);
  });
});
