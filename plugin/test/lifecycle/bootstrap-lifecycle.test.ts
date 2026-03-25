import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createPluginSimulator, resetPluginState } from "./plugin-simulator.js";
import { writeState } from "../../src/state/plugin-state.js";
import type { A2exPluginState } from "../../src/state/plugin-state.js";
import {
  TOOL_SYSTEM_HEALTH,
  TOOL_BOOTSTRAP,
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

describe("bootstrap lifecycle: factory tool exposure + turn transition", () => {
  let stateDir: string;

  beforeEach(async () => {
    resetPluginState();
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-bootstrap-lifecycle-"));
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true });
  });

  it("not_initialized phase exposes [system_health, bootstrap] (2 tools)", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    expect(tools!).toHaveLength(2);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
    expect(names).toContain(TOOL_BOOTSTRAP);
  });

  it("bootstrapping phase also exposes bootstrap for re-invocation", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // Simulate partial bootstrap
    await writeState(stateDir, {
      phase: "bootstrapping",
      waiaasPort: 3100,
      waiaasPid: 12345,
      vaultWalletId: "vault-id",
      vaultAddress: "0xVAULT",
      lastUpdated: "",
    });

    const tools = sim.resolveTools();
    expect(tools).not.toBeNull();
    expect(tools!).toHaveLength(2);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
    expect(names).toContain(TOOL_BOOTSTRAP);
  });

  it("bootstrapped phase exposes only system_health (1 tool)", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    await writeState(stateDir, {
      phase: "bootstrapped",
      waiaasPort: 3100,
      waiaasPid: 12345,
      vaultWalletId: "vault-id",
      hotWalletId: "hot-id",
      lastUpdated: "",
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

  it("turn transition: tool count changes from not_initialized (2) to bootstrapped (8)", async () => {
    const sim = createPluginSimulator();
    sim.register();
    await sim.startServices(stateDir);

    // Turn 1: not_initialized → 2 tools
    const turn1 = sim.resolveTools()!;
    expect(turn1).toHaveLength(2);
    expect(turn1.map((t) => t.name)).toContain(TOOL_BOOTSTRAP);

    // Simulate bootstrap completion
    const bootstrappedState: A2exPluginState = {
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
    await writeState(stateDir, bootstrappedState);

    // Turn 2: bootstrapped → 8 tools (system_health + 7 waiaas; bootstrap no longer exposed)
    const turn2 = sim.resolveTools()!;
    expect(turn2).toHaveLength(8);
    const turn2Names = turn2.map((t) => t.name);
    expect(turn2Names).toContain(TOOL_SYSTEM_HEALTH);
    expect(turn2Names).not.toContain(TOOL_BOOTSTRAP);
    for (const waiaasName of ALL_WAIAAS_TOOL_NAMES) {
      expect(turn2Names).toContain(waiaasName);
    }

    // Verify system_health reflects bootstrapped state
    const result = (await turn2[0].execute("test", {})) as {
      content: Array<{ type: string; text: string }>;
    };
    const parsed = JSON.parse(result.content[0].text);
    expect(parsed.status).toBe("bootstrapped");
    expect(parsed.wallets.vault).toBe("vault-id");
    expect(parsed.wallets.hot).toBe("hot-id");
  });
});
