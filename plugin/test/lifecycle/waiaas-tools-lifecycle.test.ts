import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  createPluginSimulator,
  resetPluginState,
} from "./plugin-simulator.js";
import { writeState } from "../../src/state/plugin-state.js";
import type { A2exPluginState } from "../../src/state/plugin-state.js";
import {
  TOOL_SYSTEM_HEALTH,
  TOOL_WAIAAS_GET_BALANCE,
  TOOL_WAIAAS_GET_ADDRESS,
  TOOL_WAIAAS_CALL_CONTRACT,
  TOOL_WAIAAS_SEND_TOKEN,
  TOOL_WAIAAS_GET_TRANSACTION,
  TOOL_WAIAAS_SIGN_MESSAGE,
  TOOL_WAIAAS_LIST_TRANSACTIONS,
} from "../../src/constants.js";

const ALL_WAIAAS_TOOL_NAMES = [
  TOOL_WAIAAS_GET_BALANCE,
  TOOL_WAIAAS_GET_ADDRESS,
  TOOL_WAIAAS_CALL_CONTRACT,
  TOOL_WAIAAS_SEND_TOKEN,
  TOOL_WAIAAS_GET_TRANSACTION,
  TOOL_WAIAAS_SIGN_MESSAGE,
  TOOL_WAIAAS_LIST_TRANSACTIONS,
] as const;

/** Helper to build a fully-populated bootstrapped state. */
function makeBootstrappedState(): A2exPluginState {
  return {
    phase: "bootstrapped",
    waiaasPort: 3100,
    waiaasPid: 12345,
    vaultWalletId: "vault-id",
    vaultAddress: "0xVAULT",
    hotWalletId: "hot-id",
    hotAddress: "0xHOT",
    vaultSessionToken: "vault-token",
    hotSessionToken: "hot-token",
    policyIds: { vault: "vault-policy-id", hot: "hot-policy-id", hotWhitelistArb: "arb-wl", hotWhitelistPoly: "poly-wl" },
    lastUpdated: "",
  };
}

describe("waiaas tools lifecycle: factory phase → tool exposure", () => {
  let stateDir: string;

  beforeEach(async () => {
    resetPluginState();
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-waiaas-lifecycle-"));
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true });
  });

  it("bootstrapped phase → 8 tools (system_health + 7 waiaas)", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    await writeState(stateDir, makeBootstrappedState());

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    expect(tools!).toHaveLength(8);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
    for (const waiaasName of ALL_WAIAAS_TOOL_NAMES) {
      expect(names).toContain(waiaasName);
    }
  });

  it("running phase → 8 tools (system_health + 7 waiaas)", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    await writeState(stateDir, {
      ...makeBootstrappedState(),
      phase: "running",
    });

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    expect(tools!).toHaveLength(8);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
    for (const waiaasName of ALL_WAIAAS_TOOL_NAMES) {
      expect(names).toContain(waiaasName);
    }
  });

  it("not_initialized phase → 2 tools only (no waiaas)", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // No state written → not_initialized
    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    expect(tools!).toHaveLength(2);

    const names = tools!.map((t) => t.name);
    for (const waiaasName of ALL_WAIAAS_TOOL_NAMES) {
      expect(names).not.toContain(waiaasName);
    }
  });

  it("turn transition: bootstrapping (2 tools) → bootstrapped (8 tools with waiaas)", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // Turn 1: bootstrapping → 2 tools, no waiaas
    await writeState(stateDir, {
      phase: "bootstrapping",
      waiaasPort: 3100,
      waiaasPid: 12345,
      vaultWalletId: "vault-id",
      vaultAddress: "0xVAULT",
      lastUpdated: "",
    });

    const turn1 = sim.resolveTools()!;
    expect(turn1).toHaveLength(2);
    const turn1Names = turn1.map((t) => t.name);
    for (const waiaasName of ALL_WAIAAS_TOOL_NAMES) {
      expect(turn1Names).not.toContain(waiaasName);
    }

    // Simulate bootstrap completion
    await writeState(stateDir, makeBootstrappedState());

    // Turn 2: bootstrapped → 8 tools, all waiaas present
    const turn2 = sim.resolveTools()!;
    expect(turn2).toHaveLength(8);
    const turn2Names = turn2.map((t) => t.name);
    expect(turn2Names).toContain(TOOL_SYSTEM_HEALTH);
    for (const waiaasName of ALL_WAIAAS_TOOL_NAMES) {
      expect(turn2Names).toContain(waiaasName);
    }
  });
});
