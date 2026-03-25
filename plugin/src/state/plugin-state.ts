import { readFile, writeFile, mkdir, rename } from "node:fs/promises";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { STATE_SUBDIR, STATE_FILENAME } from "../constants.js";

/**
 * Canonical runtime state for the A2EX plugin.
 * Persisted to `${stateDir}/a2ex/a2ex-state.json`.
 * Every field needed by S02–S05 is declared here upfront.
 */
export interface A2exPluginState {
  phase:
    | "not_initialized"
    | "bootstrapping"
    | "bootstrapped"
    | "running";
  waiaasPid?: number;
  a2exPid?: number;
  waiaasDataDir?: string;
  vaultWalletId?: string;
  hotWalletId?: string;
  vaultSessionToken?: string;
  hotSessionToken?: string;
  policyIds?: {
    vault?: string;
    hot?: string;
    hotContractWhitelist?: string; // legacy
    hotWhitelistArb?: string;
    hotWhitelistPoly?: string;
  };
  vaultAddress?: string;
  hotAddress?: string;
  binaryPath?: string;
  waiaasPort: number;
  lastUpdated: string; // ISO-8601
  /** Persisted for WAIaaS restart & keystore recovery. */
  masterPassword?: string;
}

/**
 * Read persisted plugin state.
 * Returns `null` on missing file or corrupt JSON — never throws.
 */
export async function readState(
  stateDir: string,
): Promise<A2exPluginState | null> {
  const filePath = join(stateDir, STATE_SUBDIR, STATE_FILENAME);
  try {
    const raw = await readFile(filePath, "utf-8");
    return JSON.parse(raw) as A2exPluginState;
  } catch {
    return null;
  }
}

/**
 * Atomically write plugin state.
 * Creates intermediate directories if needed.
 * Uses tmp-file + rename for crash-safety.
 * Stamps `lastUpdated` with the current ISO timestamp.
 */
export async function writeState(
  stateDir: string,
  state: A2exPluginState,
): Promise<void> {
  const dir = join(stateDir, STATE_SUBDIR);
  await mkdir(dir, { recursive: true });

  const stamped: A2exPluginState = {
    ...state,
    lastUpdated: new Date().toISOString(),
  };

  const tmpName = join(
    dir,
    `.${STATE_FILENAME}.${randomBytes(4).toString("hex")}.tmp`,
  );
  const target = join(dir, STATE_FILENAME);

  await writeFile(tmpName, JSON.stringify(stamped, null, 2), "utf-8");
  await rename(tmpName, target);
}
