/**
 * Mock MCP server for testing.
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

const server = new McpServer({
  name: "a2ex-mock",
  version: "0.1.0",
});

server.tool(
  "onboarding.bootstrap_install",
  "Install a2ex binary from the given URL",
  { url: z.string().describe("Download URL for the a2ex binary") },
  async ({ url }) => ({
    content: [
      {
        type: "text",
        text: JSON.stringify({ ok: true, installed: url }),
      },
    ],
  }),
);

server.tool(
  "skills.load_bundle",
  "Load a skill bundle by entry URL",
  { entry_url: z.string().describe("Entry URL of the skill bundle") },
  async ({ entry_url }) => ({
    content: [
      {
        type: "text",
        text: JSON.stringify({ ok: true, loaded: entry_url }),
      },
    ],
  }),
);

server.tool(
  "runtime.stop",
  "Gracefully stop the a2ex runtime",
  async () => ({
    content: [
      {
        type: "text",
        text: JSON.stringify({ ok: true, stopped: true }),
      },
    ],
  }),
);

const transport = new StdioServerTransport();
await server.connect(transport);
