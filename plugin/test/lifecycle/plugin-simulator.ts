/**
 * Minimal OpenClaw plugin simulator.
 *
 * Reproduces the host's register → start → resolveTools lifecycle
 * so L1.5 tests can verify per-turn factory behavior without running
 * the full OpenClaw runtime.
 *
 * Modeled after:
 * - openclaw/src/plugins/registry.ts — registerTool / registerService
 * - openclaw/src/plugins/services.ts — startPluginServices
 * - openclaw/src/plugins/tools.ts   — resolvePluginTools
 */

import register, { __resetForTesting } from "../../src/index.js";
import type {
  AnyAgentTool,
  OpenClawPluginApi,
  RegisterToolOpts,
  OpenClawPluginService,
  OpenClawPluginToolFactory,
  OpenClawPluginToolContext,
} from "../../src/types/openclaw-plugin.js";

export interface PluginSimulator {
  /** Call the plugin's register(api) to collect service and tool factory registrations. */
  register(): void;

  /** Start all registered services with the given stateDir. */
  startServices(stateDir: string): Promise<void>;

  /** Stop all registered services. */
  stopServices(): Promise<void>;

  /**
   * Resolve tools for a simulated agent turn.
   *
   * Calls each registered tool factory with a mock ToolContext,
   * collects results, and flattens into a single tool array.
   * Returns null if no tools are available (all factories returned null).
   *
   * This reproduces the pattern from openclaw/src/plugins/tools.ts:
   * resolvePluginTools iterates registered factories, calls each,
   * filters nulls, and flattens the result.
   */
  resolveTools(): AnyAgentTool[] | null;
}

/**
 * Creates a plugin simulator that loads the A2EX plugin's register function.
 *
 * Usage:
 * ```ts
 * const sim = createPluginSimulator();
 * sim.register();                   // loads plugin
 * await sim.startServices(tmpDir);  // boots services
 * const tools = sim.resolveTools(); // simulates an agent turn
 * ```
 */
/** Reset shared module state for test isolation. */
export function resetPluginState(): void {
  __resetForTesting();
}

export function createPluginSimulator(): PluginSimulator {
  const services: OpenClawPluginService[] = [];
  const toolFactories: Array<{
    factory: OpenClawPluginToolFactory;
    opts?: RegisterToolOpts;
  }> = [];
  const staticTools: Array<{
    tool: AnyAgentTool;
    opts?: RegisterToolOpts;
  }> = [];

  /** Mock OpenClawPluginApi — captures registrations. */
  const mockApi: OpenClawPluginApi = {
    registerService(service: OpenClawPluginService) {
      services.push(service);
    },

    registerTool(
      toolOrFactory:
        | AnyAgentTool
        | AnyAgentTool[]
        | OpenClawPluginToolFactory,
      opts?: RegisterToolOpts,
    ) {
      if (typeof toolOrFactory === "function") {
        toolFactories.push({ factory: toolOrFactory, opts });
      } else if (Array.isArray(toolOrFactory)) {
        staticTools.push(...toolOrFactory.map((tool) => ({ tool, opts })));
      } else {
        staticTools.push({ tool: toolOrFactory, opts });
      }
    },
  };

  return {
    register() {
      register(mockApi);
    },

    async startServices(stateDir: string) {
      for (const service of services) {
        await service.start({ stateDir });
      }
    },

    async stopServices() {
      for (const service of services) {
        await service.stop();
      }
    },

    resolveTools(): AnyAgentTool[] | null {
      const allTools: AnyAgentTool[] = staticTools.map(({ tool }) => tool);

      // Reproduce resolvePluginTools pattern from openclaw/src/plugins/tools.ts
      const mockCtx: OpenClawPluginToolContext = {};

      for (const { factory, opts } of toolFactories) {
        try {
          const result = factory(mockCtx);
          if (result == null) continue;
          const resolved = Array.isArray(result) ? result : [result];
          const allowlist = opts?.names ?? null;
          const filtered = allowlist == null
            ? resolved
            : resolved.filter((tool) => allowlist.includes(tool.name));
          allTools.push(...filtered);
        } catch {
          // OpenClaw swallows factory exceptions — plugin gets no tools
          continue;
        }
      }

      return allTools.length > 0 ? allTools : null;
    },
  };
}
