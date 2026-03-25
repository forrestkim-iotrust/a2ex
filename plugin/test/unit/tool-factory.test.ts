import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createToolFactory } from "../../src/tools/factory.js";
import {
  TOOL_SYSTEM_HEALTH,
  TOOL_WAIAAS_GET_BALANCE,
  TOOL_WAIAAS_GET_ADDRESS,
  TOOL_WAIAAS_CALL_CONTRACT,
  TOOL_WAIAAS_SEND_TOKEN,
  TOOL_WAIAAS_GET_TRANSACTION,
  TOOL_WAIAAS_SIGN_MESSAGE,
  TOOL_WAIAAS_LIST_TRANSACTIONS,
  STATE_SUBDIR,
  STATE_FILENAME,
} from "../../src/constants.js";

describe("createToolFactory", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-test-"));
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true });
  });

  it("returns bootstrap + health tools when stateDir has not been captured", () => {
    const factory = createToolFactory(() => null);
    const result = factory({});
    expect(Array.isArray(result)).toBe(true);
    expect(result!.length).toBe(2);
    const names = result!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
  });

  it("returns system_health tool when stateDir exists but no state file", () => {
    const factory = createToolFactory(() => stateDir);
    const tools = factory({});
    expect(tools).not.toBeNull();
    expect(Array.isArray(tools)).toBe(true);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
  });

  it("returns system_health when state phase is not_initialized", async () => {
    await mkdir(join(stateDir, STATE_SUBDIR), { recursive: true });
    await writeFile(
      join(stateDir, STATE_SUBDIR, STATE_FILENAME),
      JSON.stringify({
        phase: "not_initialized",
        waiaasPort: 3100,
        lastUpdated: new Date().toISOString(),
      }),
      "utf-8",
    );

    const factory = createToolFactory(() => stateDir);
    const tools = factory({});
    expect(tools).not.toBeNull();

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
  });

  it("returns 8 tools (system_health + 7 waiaas) when state phase is bootstrapped", async () => {
    await mkdir(join(stateDir, STATE_SUBDIR), { recursive: true });
    await writeFile(
      join(stateDir, STATE_SUBDIR, STATE_FILENAME),
      JSON.stringify({
        phase: "bootstrapped",
        waiaasPort: 3100,
        waiaasPid: 424242,
        lastUpdated: new Date().toISOString(),
      }),
      "utf-8",
    );

    const factory = createToolFactory(() => stateDir);
    const tools = factory({});
    expect(tools).not.toBeNull();
    expect(tools).toHaveLength(8);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
    expect(names).toContain(TOOL_WAIAAS_GET_BALANCE);
    expect(names).toContain(TOOL_WAIAAS_GET_ADDRESS);
    expect(names).toContain(TOOL_WAIAAS_CALL_CONTRACT);
    expect(names).toContain(TOOL_WAIAAS_SEND_TOKEN);
    expect(names).toContain(TOOL_WAIAAS_GET_TRANSACTION);
    expect(names).toContain(TOOL_WAIAAS_SIGN_MESSAGE);
    expect(names).toContain(TOOL_WAIAAS_LIST_TRANSACTIONS);
  });

  it("returns 8 tools (system_health + 7 waiaas) when state phase is running", async () => {
    await mkdir(join(stateDir, STATE_SUBDIR), { recursive: true });
    await writeFile(
      join(stateDir, STATE_SUBDIR, STATE_FILENAME),
      JSON.stringify({
        phase: "running",
        waiaasPort: 3100,
        waiaasPid: 424242,
        lastUpdated: new Date().toISOString(),
      }),
      "utf-8",
    );

    const factory = createToolFactory(() => stateDir);
    const tools = factory({});
    expect(tools).not.toBeNull();
    expect(tools).toHaveLength(8);

    const names = tools!.map((t) => t.name);
    expect(names).toContain(TOOL_SYSTEM_HEALTH);
    expect(names).toContain(TOOL_WAIAAS_GET_BALANCE);
  });

  it("catches exceptions and returns empty array (never throws)", () => {
    const factory = createToolFactory(() => {
      throw new Error("boom");
    });
    const result = factory({});
    expect(Array.isArray(result)).toBe(true);
    expect(result).toHaveLength(0);
  });
});
