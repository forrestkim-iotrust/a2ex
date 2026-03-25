import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createSystemHealthTool } from "../../src/tools/system-health.js";
import { writeState, type A2exPluginState } from "../../src/state/plugin-state.js";
import { TOOL_SYSTEM_HEALTH } from "../../src/constants.js";

/** Helper to unwrap the MCP content envelope and parse the JSON text. */
function unwrap(result: unknown): Record<string, unknown> {
  const r = result as { content: { type: string; text: string }[] };
  return JSON.parse(r.content[0].text);
}

describe("system_health tool", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-health-"));
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true });
  });

  it("has the correct tool name and AnyAgentTool shape", () => {
    const tool = createSystemHealthTool(() => null);
    expect(tool.name).toBe(TOOL_SYSTEM_HEALTH);
    expect(tool.description).toBeTruthy();
    expect(tool.parameters).toEqual({ type: "object", properties: {} });
    expect(typeof tool.execute).toBe("function");
  });

  it("returns not_initialized when stateDir is null", async () => {
    const tool = createSystemHealthTool(() => null);
    const result = unwrap(await tool.execute("test", {}));
    expect(result.status).toBe("not_initialized");
    expect(result.message).toBe("Plugin service not started");
  });

  it("returns not_initialized when state file does not exist", async () => {
    const tool = createSystemHealthTool(() => stateDir);
    const result = unwrap(await tool.execute("test", {}));
    expect(result.status).toBe("not_initialized");
    expect(result.message).toBe("Bootstrap required");
  });

  it("returns bootstrapped status with state details", async () => {
    const state: A2exPluginState = {
      phase: "bootstrapped",
      waiaasPort: 3100,
      waiaasPid: 1234,
      vaultWalletId: "vault-abc",
      hotWalletId: "hot-def",
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, state);

    const tool = createSystemHealthTool(() => stateDir);
    const result = unwrap(await tool.execute("test", {}));

    expect(result.status).toBe("bootstrapped");
    expect(result.waiaas).toEqual({
      pid: 1234,
      port: 3100,
      dataDir: undefined,
      running: false,
    });
    expect(result.wallets).toEqual({ vault: "vault-abc", hot: "hot-def" });
  });

  it("returns running status with PID info", async () => {
    const state: A2exPluginState = {
      phase: "running",
      waiaasPort: 3100,
      waiaasPid: 5555,
      vaultWalletId: "v1",
      hotWalletId: "h1",
      lastUpdated: new Date().toISOString(),
    };
    await writeState(stateDir, state);

    const tool = createSystemHealthTool(() => stateDir);
    const result = unwrap(await tool.execute("test", {}));

    expect(result.status).toBe("running");
    expect(result.a2ex).toEqual({ connected: false, toolCount: 0 });
    expect(result.waiaas).toEqual({
      pid: 5555,
      port: 3100,
      dataDir: undefined,
      running: false,
    });
  });
});
