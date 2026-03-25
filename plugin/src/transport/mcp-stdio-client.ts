/**
 * MCP stdio client adapter.
 *
 * Wraps the @modelcontextprotocol/sdk Client + StdioClientTransport
 * to provide a focused interface for the A2EX plugin:
 *   - spawn an MCP server subprocess
 *   - connect via stdio transport
 *   - list available tools
 *   - call a tool by name with args
 *   - close and clean up the subprocess
 */

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import type { Tool } from "@modelcontextprotocol/sdk/types.js";

// ---------------------------------------------------------------------------
// Custom error classes
// ---------------------------------------------------------------------------

/** Thrown when the MCP stdio connection/handshake fails. */
export class McpConnectionError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "McpConnectionError";
  }
}

/** Thrown when a tool call over MCP fails. */
export class McpCallError extends Error {
  readonly toolName: string;

  constructor(toolName: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "McpCallError";
    this.toolName = toolName;
  }
}

// ---------------------------------------------------------------------------
// Client options
// ---------------------------------------------------------------------------

export interface McpStdioClientOptions {
  /** Path to the executable (e.g. a2ex binary). */
  command: string;
  /** CLI args passed to the command. */
  args?: string[];
  /** Extra environment variables for the subprocess. */
  env?: Record<string, string>;
  /** Working directory for the subprocess. */
  cwd?: string;
  /** Called when the MCP connection drops unexpectedly (not via close()). */
  onClose?: () => void;
}

// ---------------------------------------------------------------------------
// McpStdioClient
// ---------------------------------------------------------------------------

/**
 * High-level MCP client that communicates with a server over stdio.
 *
 * Lifecycle: construct → connect() → listTools() / callTool() → close()
 */
export class McpStdioClient {
  private readonly opts: McpStdioClientOptions;
  private client: Client | null = null;
  private transport: StdioClientTransport | null = null;
  private connected = false;

  constructor(opts: McpStdioClientOptions) {
    this.opts = opts;
  }

  /** Whether the client has an active MCP session. */
  get isConnected(): boolean {
    return this.connected;
  }

  /**
   * Spawn the subprocess and perform the MCP initialize handshake.
   * @throws {McpConnectionError} if spawn or handshake fails.
   */
  async connect(): Promise<void> {
    if (this.connected) return;

    try {
      this.transport = new StdioClientTransport({
        command: this.opts.command,
        args: this.opts.args,
        env: { ...process.env, ...this.opts.env } as Record<string, string>,
        ...(this.opts.cwd ? { cwd: this.opts.cwd } : {}),
        stderr: "pipe",
      });

      this.client = new Client(
        { name: "openclaw-plugin-a2ex", version: "0.0.1" },
        { capabilities: {} },
      );

      await this.client.connect(this.transport);
      this.connected = true;

      // Listen for unexpected transport close
      if (this.opts.onClose) {
        const onCloseCallback = this.opts.onClose;
        this.transport.onclose = () => {
          // Only fire if the close was unexpected (not via our close() method)
          if (this.connected) {
            this.connected = false;
            this.client = null;
            this.transport = null;
            onCloseCallback();
          }
        };
      }
    } catch (err) {
      // Clean up partial state on failure
      this.client = null;
      this.transport = null;
      this.connected = false;
      throw new McpConnectionError(
        `Failed to connect to MCP server: ${this.opts.command} ${(this.opts.args ?? []).join(" ")}`.trim(),
        { cause: err },
      );
    }
  }

  /**
   * Request the server's tool list via `tools/list`.
   * @returns Array of MCP Tool descriptors.
   */
  async listTools(): Promise<Tool[]> {
    this.assertConnected();
    const result = await this.client!.listTools();
    return result.tools;
  }

  /**
   * Invoke a tool on the server via `tools/call`.
   * @param name  Fully-qualified tool name (e.g. `onboarding.bootstrap_install`).
   * @param args  JSON-serializable arguments matching the tool's inputSchema.
   * @returns The CallToolResult from the server.
   * @throws {McpCallError} if the tool invocation fails.
   */
  async callTool(
    name: string,
    args: Record<string, unknown> = {},
  ): Promise<unknown> {
    this.assertConnected();

    try {
      const result = await this.client!.callTool({ name, arguments: args });
      return result;
    } catch (err) {
      throw new McpCallError(
        name,
        `Tool call failed: ${name}`,
        { cause: err },
      );
    }
  }

  /**
   * Close the MCP session and terminate the subprocess.
   * Safe to call multiple times.
   */
  async close(): Promise<void> {
    if (!this.connected && !this.client) return;

    try {
      await this.client?.close();
    } finally {
      this.client = null;
      this.transport = null;
      this.connected = false;
    }
  }

  private assertConnected(): void {
    if (!this.connected || !this.client) {
      throw new McpConnectionError("Not connected to MCP server");
    }
  }
}
