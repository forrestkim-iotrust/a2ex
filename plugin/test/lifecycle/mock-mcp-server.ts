#!/usr/bin/env npx tsx
/**
 * Mock MCP server for testing.
 *
 * Registers 3 sample tools that mirror the A2EX daemon's eventual MCP surface:
 *   - onboarding.bootstrap_install (string param `url`, echo response)
 *   - skills.load_bundle (string param `entry_url`)
 *   - runtime.stop (no params)
 *
 * Run directly: `npx tsx test/lifecycle/mock-mcp-server.ts`
 * The process communicates via stdio using the MCP JSON-RPC protocol.
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

const server = new McpServer({
  name: "a2ex-mock",
  version: "0.1.0",
});

// --- Tool: onboarding.bootstrap_install ---
server.tool(
  "onboarding.bootstrap_install",
  "Install a2ex binary from the given URL",
  { url: z.string().describe("Download URL for the a2ex binary") },
  async ({ url }) => ({
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({ ok: true, installed: url }),
      },
    ],
  }),
);

// --- Tool: skills.load_bundle ---
server.tool(
  "skills.load_bundle",
  "Load a skill bundle by entry URL",
  { entry_url: z.string().describe("Entry URL of the skill bundle") },
  async ({ entry_url }) => ({
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({ ok: true, loaded: entry_url }),
      },
    ],
  }),
);

// --- Tool: runtime.stop ---
server.tool(
  "runtime.stop",
  "Gracefully stop the a2ex runtime",
  async () => ({
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({ ok: true, stopped: true }),
      },
    ],
  }),
);

// --- Start serving over stdio ---
const transport = new StdioServerTransport();
await server.connect(transport);
