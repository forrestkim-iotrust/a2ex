/**
 * L1 unit tests for McpStdioClient.
 *
 * These are intentionally RED in T01 — the skeleton throws "not implemented".
 * T02 will make them green by implementing the client.
 *
 * Tests spawn the mock MCP server as a real subprocess and verify:
 *   (a) connect → listTools → 3 tools returned
 *   (b) callTool → expected response
 *   (c) close → subprocess terminated
 */

import { describe, it, expect, afterEach } from "vitest";
import { resolve } from "node:path";
import {
  McpStdioClient,
  McpConnectionError,
  McpCallError,
} from "../../src/transport/mcp-stdio-client.js";

const MOCK_SERVER_PATH = resolve(
  import.meta.dirname,
  "../lifecycle/mock-mcp-server.mjs",
);

describe("McpStdioClient", () => {
  let client: McpStdioClient;

  afterEach(async () => {
    try {
      await client?.close();
    } catch {
      // ignore cleanup errors
    }
  });

  it("connects to mock MCP server and lists 3 tools", async () => {
    client = new McpStdioClient({
      command: process.execPath,
      args: [MOCK_SERVER_PATH],
    });

    await client.connect();
    const tools = await client.listTools();

    expect(tools).toHaveLength(3);

    const names = tools.map((t) => t.name).sort();
    expect(names).toEqual([
      "onboarding.bootstrap_install",
      "runtime.stop",
      "skills.load_bundle",
    ]);
  });

  it("calls onboarding.bootstrap_install and gets echo response", async () => {
    client = new McpStdioClient({
      command: process.execPath,
      args: [MOCK_SERVER_PATH],
    });

    await client.connect();
    const result = await client.callTool("onboarding.bootstrap_install", {
      url: "https://example.com/a2ex-v1.0.0",
    });

    // The result should contain the echoed URL
    expect(result).toBeDefined();
    const text =
      typeof result === "string"
        ? result
        : JSON.stringify(result);
    expect(text).toContain("https://example.com/a2ex-v1.0.0");
  });

  it("close() terminates the subprocess", async () => {
    client = new McpStdioClient({
      command: process.execPath,
      args: [MOCK_SERVER_PATH],
    });

    await client.connect();
    await client.close();

    // After close, calling listTools should throw or reject
    await expect(client.listTools()).rejects.toThrow();
  });

  it("throws McpConnectionError for nonexistent binary", async () => {
    client = new McpStdioClient({
      command: "/nonexistent/binary/path",
      args: [],
    });

    await expect(client.connect()).rejects.toThrow(McpConnectionError);
  });

  it("exports McpConnectionError and McpCallError", () => {
    const connErr = new McpConnectionError("test");
    expect(connErr).toBeInstanceOf(Error);
    expect(connErr.name).toBe("McpConnectionError");

    const callErr = new McpCallError("someTool", "test");
    expect(callErr).toBeInstanceOf(Error);
    expect(callErr.name).toBe("McpCallError");
    expect(callErr.toolName).toBe("someTool");
  });
});
