/**
 * Dynamic tool generation from MCP tools/list results.
 *
 * Converts MCP Tool[] into AnyAgentTool[] with `a2ex.` prefix,
 * enabling runtime-discovered tools to be exposed to OpenClaw agents.
 *
 * Uses a module-level cache so the tool factory can return tools
 * synchronously while the MCP connection refreshes asynchronously.
 */

import type { Tool } from "@modelcontextprotocol/sdk/types.js";
import type { JSONSchema7 } from "../types/json-schema.js";
import type { AnyAgentTool } from "../types/openclaw-plugin.js";
import type { McpStdioClient } from "../transport/mcp-stdio-client.js";
import { A2EX_TOOL_PREFIX } from "../constants.js";

// ---------------------------------------------------------------------------
// Module-level cache
// ---------------------------------------------------------------------------

let cachedTools: Tool[] | null = null;
let cachedClient: McpStdioClient | null = null;

/**
 * Replace the cached MCP tool descriptors and client reference.
 * Called after a successful `tools/list` from the MCP client.
 */
export function setMcpCache(tools: Tool[], client: McpStdioClient): void {
  cachedTools = tools;
  cachedClient = client;
}

/**
 * Read the current cached state.
 * Returns `null` if no cache has been set (MCP not connected yet).
 */
export function getMcpCache(): { tools: Tool[]; client: McpStdioClient } | null {
  if (cachedTools == null || cachedClient == null) return null;
  return { tools: cachedTools, client: cachedClient };
}

/**
 * Clear the cached tools and client (e.g. on MCP disconnect or process exit).
 */
export function clearMcpCache(): void {
  cachedTools = null;
  cachedClient = null;
}

/**
 * Return AnyAgentTool[] wrappers for cached MCP tools with `a2ex.` prefix.
 *
 * Each wrapper, when executed, calls the corresponding MCP tool via the
 * cached client's callTool method.
 *
 * Returns empty array if cache is not set (graceful degradation).
 */
export function getA2exDynamicTools(): AnyAgentTool[] {
  if (cachedTools == null || cachedClient == null) return [];

  // Capture client ref for the closure — avoids stale reference if cache is cleared
  const client = cachedClient;

  return cachedTools.map((tool): AnyAgentTool => ({
    name: `${A2EX_TOOL_PREFIX}${tool.name}`,
    description: tool.description ?? "",
    parameters: tool.inputSchema as JSONSchema7,
    execute: async (_toolCallId: string, params: Record<string, unknown>) => {
      return client.callTool(tool.name, params);
    },
  }));
}

/**
 * Reset all cache state. For test isolation only.
 */
export function __resetCacheForTesting(): void {
  cachedTools = null;
  cachedClient = null;
}
