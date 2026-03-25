import { TOOL_SYSTEM_HEALTH } from "../constants.js";
import { readState } from "../state/plugin-state.js";
import { getA2exDynamicTools } from "./a2ex-dynamic.js";
import type { AnyAgentTool } from "../types/openclaw-plugin.js";
import { isWaiaasRunning } from "../services/waiaas.service.js";

/**
 * Creates the `a2ex.system_health` diagnostic tool.
 *
 * This is the primary inspection surface for the plugin — any future agent
 * (or human) can call it to see current phase, process PIDs, wallet IDs,
 * and connectivity status at a glance.
 *
 * @param getStateDir — thunk returning the captured stateDir, or null if the
 *   service hasn't started yet (stateDir not yet captured).
 */
export function createSystemHealthTool(
  getStateDir: () => string | null,
): AnyAgentTool {
  return {
    name: TOOL_SYSTEM_HEALTH,
    description:
      "Returns the current health and status of the A2EX plugin, " +
      "including process PIDs, wallet IDs, and lifecycle phase.",
    parameters: { type: "object", properties: {} },

    async execute(_toolCallId: string) {
      const stateDir = getStateDir();

      if (stateDir == null) {
        return wrap({
          status: "not_initialized",
          message: "Plugin service not started",
        });
      }

      const state = await readState(stateDir);

      if (state == null) {
        return wrap({
          status: "not_initialized",
          message: "Bootstrap required",
        });
      }

      return wrap({
        status: state.phase,
        waiaas: {
          pid: state.waiaasPid,
          port: state.waiaasPort,
          dataDir: state.waiaasDataDir,
          running: state.waiaasPid != null ? isWaiaasRunning(state.waiaasPid) : false,
        },
        a2ex: {
          connected: getA2exDynamicTools().length > 0,
          toolCount: getA2exDynamicTools().length,
        },
        wallets: {
          vault: state.vaultWalletId,
          hot: state.hotWalletId,
        },
      });
    },
  };
}

/** Wrap a result object in the MCP content envelope. */
function wrap(result: Record<string, unknown>) {
  return { content: [{ type: "text", text: JSON.stringify(result) }] };
}
