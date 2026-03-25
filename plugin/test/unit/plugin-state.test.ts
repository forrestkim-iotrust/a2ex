import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  readState,
  writeState,
  type A2exPluginState,
} from "../../src/state/plugin-state.js";
import { STATE_SUBDIR, STATE_FILENAME } from "../../src/constants.js";

const baseState: A2exPluginState = {
  phase: "bootstrapped",
  waiaasPort: 3100,
  lastUpdated: "2025-01-01T00:00:00.000Z",
};

describe("readState", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-test-"));
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true });
  });

  it("returns null when state file does not exist", async () => {
    const result = await readState(stateDir);
    expect(result).toBeNull();
  });

  it("returns parsed state from valid JSON file", async () => {
    const stateSubdir = join(stateDir, STATE_SUBDIR);
    await mkdir(stateSubdir, { recursive: true });
    const statePath = join(stateSubdir, STATE_FILENAME);
    await writeFile(statePath, JSON.stringify(baseState), "utf-8");

    const result = await readState(stateDir);
    expect(result).toEqual(baseState);
  });

  it("returns null when state file contains corrupt JSON", async () => {
    const stateSubdir = join(stateDir, STATE_SUBDIR);
    await mkdir(stateSubdir, { recursive: true });
    const statePath = join(stateSubdir, STATE_FILENAME);
    await writeFile(statePath, "not valid json{{{", "utf-8");

    const result = await readState(stateDir);
    expect(result).toBeNull();
  });
});

describe("writeState", () => {
  let stateDir: string;

  beforeEach(async () => {
    stateDir = await mkdtemp(join(tmpdir(), "a2ex-test-"));
  });

  afterEach(async () => {
    await rm(stateDir, { recursive: true, force: true });
  });

  it("creates state file with valid JSON content and stamps lastUpdated", async () => {
    const before = new Date().toISOString();
    await writeState(stateDir, baseState);
    const after = new Date().toISOString();

    const result = await readState(stateDir);
    expect(result).not.toBeNull();
    expect(result!.phase).toBe("bootstrapped");
    expect(result!.waiaasPort).toBe(3100);
    // lastUpdated should be stamped fresh, not the value we passed in
    expect(result!.lastUpdated >= before).toBe(true);
    expect(result!.lastUpdated <= after).toBe(true);
  });

  it("creates intermediate directories if they do not exist", async () => {
    const freshDir = join(stateDir, "nested", "deep");
    await writeState(freshDir, {
      ...baseState,
      phase: "not_initialized",
    });

    const result = await readState(freshDir);
    expect(result).not.toBeNull();
    expect(result!.phase).toBe("not_initialized");
  });

  it("writes atomically — file always contains valid JSON", async () => {
    // Write twice quickly — second write should not leave corrupt file
    await writeState(stateDir, { ...baseState, phase: "bootstrapping" });
    await writeState(stateDir, { ...baseState, phase: "running" });

    const raw = await readFile(
      join(stateDir, STATE_SUBDIR, STATE_FILENAME),
      "utf-8",
    );
    const parsed = JSON.parse(raw);
    expect(parsed.phase).toBe("running");
  });
});
