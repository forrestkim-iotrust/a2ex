/**
 * L1 unit tests for a2ex-dynamic tool generation.
 *
 * Tests verify:
 *   (a) MCP Tool[] → AnyAgentTool[] with `a2ex.` prefix via cache
 *   (b) Empty cache returns empty array (graceful degradation)
 *   (c) Cache lifecycle (set/get/clear)
 *   (d) execute delegates to cached client's callTool
 *   (e) inputSchema is passed through as parameters
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Tool } from "@modelcontextprotocol/sdk/types.js";
import type { McpStdioClient } from "../../src/transport/mcp-stdio-client.js";
import {
  getA2exDynamicTools,
  setMcpCache,
  getMcpCache,
  clearMcpCache,
  __resetCacheForTesting,
} from "../../src/tools/a2ex-dynamic.js";

// Minimal mock MCP Tool descriptors matching the mock server's output
const MOCK_MCP_TOOLS: Tool[] = [
  {
    name: "onboarding.bootstrap_install",
    description: "Install a2ex binary from the given URL",
    inputSchema: {
      type: "object" as const,
      properties: {
        url: { type: "string", description: "Download URL for the a2ex binary" },
      },
      required: ["url"],
    },
  },
  {
    name: "skills.load_bundle",
    description: "Load a skill bundle by entry URL",
    inputSchema: {
      type: "object" as const,
      properties: {
        entry_url: { type: "string", description: "Entry URL of the skill bundle" },
      },
      required: ["entry_url"],
    },
  },
  {
    name: "runtime.stop",
    description: "Gracefully stop the a2ex runtime",
    inputSchema: {
      type: "object" as const,
      properties: {},
    },
  },
];

/** Create a mock McpStdioClient with a spy on callTool. */
function createMockClient() {
  return {
    connect: vi.fn(),
    listTools: vi.fn(),
    callTool: vi.fn().mockResolvedValue({
      content: [{ type: "text", text: '{"ok":true}' }],
    }),
    close: vi.fn(),
    isConnected: true,
  } as unknown as McpStdioClient;
}

describe("getA2exDynamicTools", () => {
  beforeEach(() => {
    __resetCacheForTesting();
  });

  it("converts cached MCP tools to AnyAgentTool[] with a2ex. prefix", () => {
    const mockClient = createMockClient();
    setMcpCache(MOCK_MCP_TOOLS, mockClient);

    const tools = getA2exDynamicTools();

    expect(tools).toHaveLength(3);

    const names = tools.map((t) => t.name).sort();
    expect(names).toEqual([
      "a2ex.onboarding.bootstrap_install",
      "a2ex.runtime.stop",
      "a2ex.skills.load_bundle",
    ]);
  });

  it("each converted tool has description, parameters, and execute", () => {
    const mockClient = createMockClient();
    setMcpCache(MOCK_MCP_TOOLS, mockClient);

    const tools = getA2exDynamicTools();

    expect(tools.length).toBeGreaterThan(0);

    for (const tool of tools) {
      expect(tool.description).toBeDefined();
      expect(tool.parameters).toBeDefined();
      expect(typeof tool.execute).toBe("function");
    }
  });

  it("returns empty array when cache is not set", () => {
    const tools = getA2exDynamicTools();
    expect(tools).toEqual([]);
  });

  it("returns empty array after clearMcpCache", () => {
    const mockClient = createMockClient();
    setMcpCache(MOCK_MCP_TOOLS, mockClient);
    clearMcpCache();

    const tools = getA2exDynamicTools();
    expect(tools).toEqual([]);
  });

  it("execute delegates to cached client callTool", async () => {
    const mockClient = createMockClient();
    setMcpCache(MOCK_MCP_TOOLS, mockClient);

    const tools = getA2exDynamicTools();
    const bootstrapTool = tools.find(
      (t) => t.name === "a2ex.onboarding.bootstrap_install",
    )!;

    const params = { url: "https://example.com/a2ex-v2.0.0" };
    await bootstrapTool.execute("test", params);

    // callTool should be called with the unprefixed MCP tool name
    expect(mockClient.callTool).toHaveBeenCalledWith(
      "onboarding.bootstrap_install",
      params,
    );
  });

  it("inputSchema is passed through as parameters", () => {
    const mockClient = createMockClient();
    setMcpCache(MOCK_MCP_TOOLS, mockClient);

    const tools = getA2exDynamicTools();
    const bootstrapTool = tools.find(
      (t) => t.name === "a2ex.onboarding.bootstrap_install",
    )!;

    expect(bootstrapTool.parameters).toEqual(
      MOCK_MCP_TOOLS[0].inputSchema,
    );
  });
});

describe("MCP cache lifecycle", () => {
  beforeEach(() => {
    __resetCacheForTesting();
  });

  it("getMcpCache returns null before any set", () => {
    expect(getMcpCache()).toBeNull();
  });

  it("setMcpCache / getMcpCache round-trip", () => {
    const mockClient = createMockClient();
    setMcpCache(MOCK_MCP_TOOLS, mockClient);

    const cache = getMcpCache();
    expect(cache).not.toBeNull();
    expect(cache!.tools).toBe(MOCK_MCP_TOOLS);
    expect(cache!.client).toBe(mockClient);
  });

  it("clearMcpCache resets to null", () => {
    const mockClient = createMockClient();
    setMcpCache(MOCK_MCP_TOOLS, mockClient);
    clearMcpCache();
    expect(getMcpCache()).toBeNull();
  });
});
