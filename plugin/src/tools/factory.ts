import { readFileSync } from "node:fs";
import { join } from "node:path";
import { STATE_SUBDIR, STATE_FILENAME } from "../constants.js";
import type { A2exPluginState } from "../state/plugin-state.js";
import { createSystemHealthTool } from "./system-health.js";
import { createBootstrapTool } from "./bootstrap.js";
import { createWaiaasTools } from "./waiaas-tools.js";
import { getA2exDynamicTools } from "./a2ex-dynamic.js";
import type {
  AnyAgentTool,
  OpenClawPluginToolContext,
} from "../types/openclaw-plugin.js";

/**
 * Creates the master tool factory for the A2EX plugin.
 *
 * Called once at register-time with a `getStateDir` thunk that captures
 * the stateDir set by the service's `start()`. On every agent turn,
 * OpenClaw calls the returned factory to resolve the current tool set.
 *
 * Per-turn resolve means the tool set can change between turns based on
 * state file contents — this is the core mechanism for phased tool exposure.
 *
 * @param getStateDir — thunk returning the captured stateDir, or null if
 *   the service hasn't started yet.
 * @returns A tool factory that OpenClaw invokes per-turn.
 */
export function createToolFactory(
  getStateDir: () => string | null,
): (ctx: OpenClawPluginToolContext) => AnyAgentTool[] {
  return (_ctx: OpenClawPluginToolContext): AnyAgentTool[] => {
    try {
      const stateDir = getStateDir();

      // Service not started yet — expose bootstrap + health anyway
      if (stateDir == null) {
        return [createSystemHealthTool(getStateDir), createBootstrapTool(getStateDir)];
      }

      const state = readStateSync(stateDir);

      if (state == null || state.phase === "not_initialized") {
        // Bootstrap needed — expose diagnostic + bootstrap tools
        return [createSystemHealthTool(getStateDir), createBootstrapTool(getStateDir)];
      }

      if (state.phase === "bootstrapping") {
        // Partial bootstrap — allow re-invocation to resume
        return [createSystemHealthTool(getStateDir), createBootstrapTool(getStateDir)];
      }

      if (
        state.phase === "bootstrapped" ||
        state.phase === "running"
      ) {
        if (state.waiaasPid == null) {
          return [createSystemHealthTool(getStateDir), createBootstrapTool(getStateDir)];
        }
        return [
          createSystemHealthTool(getStateDir),
          ...createWaiaasTools(getStateDir),
          ...getA2exDynamicTools(),
        ];
      }

      // Unknown/transitional phase — expose diagnostic tool only
      return [createSystemHealthTool(getStateDir)];
    } catch {
      // Per OpenClaw convention: factory never throws, returns null on error
      return [];
    }
  };
}

/**
 * Synchronous state read for the tool factory.
 *
 * The factory must be synchronous per OpenClaw's resolvePluginTools contract
 * (tools.ts calls factories and collects return values without await).
 * Uses readFileSync instead of the async readState from plugin-state.ts.
 */
function readStateSync(stateDir: string): A2exPluginState | null {
  try {
    const filePath = join(stateDir, STATE_SUBDIR, STATE_FILENAME);
    const raw = readFileSync(filePath, "utf-8");
    return JSON.parse(raw) as A2exPluginState;
  } catch {
    return null;
  }
}
