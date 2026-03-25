/**
 * A2EX MCP subprocess lifecycle service.
 *
 * Manages the connection to the a2ex daemon via MCP stdio transport:
 *   - startA2ex: spawn + handshake + tools/list → populate dynamic tool cache
 *   - stopA2ex: clear cache + close client
 *   - isA2exRunning: check if cache is populated
 */

import { McpStdioClient } from "../transport/mcp-stdio-client.js";
import { setMcpCache, clearMcpCache, getMcpCache } from "../tools/a2ex-dynamic.js";
import { readState, writeState } from "../state/plugin-state.js";
import {
  A2EX_BACKOFF_INITIAL_MS,
  A2EX_BACKOFF_MAX_MS,
  A2EX_BACKOFF_MULTIPLIER,
} from "../constants.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface StartA2exOptions {
  /** Path to the a2ex binary (or .ts mock server in tests). */
  binaryPath: string;
  /** Plugin state directory for persisting a2ex connection info. */
  stateDir: string;
  /** Extra environment variables for the subprocess. */
  env?: Record<string, string>;
}

function resolveSubprocessCommand(binaryPath: string): {
  command: string;
  args: string[];
} {
  if (binaryPath.endsWith(".ts")) {
    return { command: "npx", args: ["tsx", binaryPath] };
  }
  if (binaryPath.endsWith(".js") || binaryPath.endsWith(".mjs")) {
    return { command: process.execPath, args: [binaryPath] };
  }
  return { command: binaryPath, args: [] };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Start the a2ex MCP subprocess and populate the dynamic tool cache.
 *
 * 1. Creates McpStdioClient (auto-detects script files for testing)
 * 2. Connects via stdio MCP handshake
 * 3. Lists available tools
 * 4. Populates the module-level cache via setMcpCache
 * 5. Updates state file with binaryPath
 *
 * On failure, cleans up client and cache before rethrowing.
 */
export async function startA2ex(options: StartA2exOptions): Promise<void> {
  const { binaryPath, stateDir, env } = options;

  const { command, args } = resolveSubprocessCommand(binaryPath);

  const client = new McpStdioClient({ command, args, env });

  try {
    await client.connect();
    const tools = await client.listTools();
    setMcpCache(tools, client);

    // Persist a2ex connection info to state
    const currentState = await readState(stateDir);
    if (currentState != null) {
      await writeState(stateDir, {
        ...currentState,
        binaryPath,
      });
    }
  } catch (err) {
    // Clean up on failure
    clearMcpCache();
    try {
      await client.close();
    } catch {
      // Swallow close errors during cleanup
    }
    throw err;
  }
}

/**
 * Stop the a2ex MCP connection and clear the dynamic tool cache.
 * Safe to call even if a2ex is not running. Errors are swallowed.
 */
export async function stopA2ex(): Promise<void> {
  const cache = getMcpCache();
  clearMcpCache();

  if (cache != null) {
    try {
      await cache.client.close();
    } catch {
      // Swallow close errors — best-effort cleanup
    }
  }
}

/**
 * Check if the a2ex MCP connection is active.
 */
export function isA2exRunning(): boolean {
  return getMcpCache() !== null;
}

// ---------------------------------------------------------------------------
// Close-event recovery with exponential backoff
// ---------------------------------------------------------------------------

export interface StartA2exWithRecoveryOptions extends StartA2exOptions {
  /** Called after each successful restart. */
  onRestart?: () => void;
}

/**
 * Start a2ex with automatic close-event recovery.
 *
 * Wraps `startA2ex` and wires an `onClose` handler that triggers
 * exponential-backoff restart: 1s → 2s → 4s → … → 30s cap.
 * Backoff resets to initial on successful restart.
 *
 * Returns a handle with `stop()` to cleanly shut down without triggering recovery.
 */
export function startA2exWithRecovery(
  options: StartA2exWithRecoveryOptions,
): { stop: () => void; start: () => Promise<void> } {
  let isStopping = false;
  let backoffMs = A2EX_BACKOFF_INITIAL_MS;
  let pendingTimer: ReturnType<typeof setTimeout> | null = null;

  const { onRestart, ...baseOptions } = options;

  async function connectWithRecovery(): Promise<void> {
    // We need to override the startA2ex internals to pass onClose.
    // Since startA2ex creates the McpStdioClient internally, we override
    // by creating the client manually here.
    const { binaryPath, stateDir, env } = baseOptions;

    const { command, args } = resolveSubprocessCommand(binaryPath);

    const client = new McpStdioClient({
      command,
      args,
      env,
      onClose: () => {
        if (isStopping) return;
        scheduleRestart();
      },
    });

    try {
      await client.connect();
      const tools = await client.listTools();
      setMcpCache(tools, client);

      // Persist a2ex connection info to state
      const currentState = await readState(stateDir);
      if (currentState != null) {
        await writeState(stateDir, {
          ...currentState,
          binaryPath,
        });
      }

      // Success — reset backoff
      backoffMs = A2EX_BACKOFF_INITIAL_MS;
    } catch (err) {
      clearMcpCache();
      try {
        await client.close();
      } catch {
        // Swallow close errors during cleanup
      }
      throw err;
    }
  }

  function scheduleRestart(): void {
    if (isStopping) return;

    const delay = backoffMs;
    backoffMs = Math.min(backoffMs * A2EX_BACKOFF_MULTIPLIER, A2EX_BACKOFF_MAX_MS);

    pendingTimer = setTimeout(async () => {
      pendingTimer = null;
      if (isStopping) return;

      clearMcpCache();

      try {
        await connectWithRecovery();
        onRestart?.();
      } catch {
        // Restart failed — schedule another attempt
        if (!isStopping) {
          scheduleRestart();
        }
      }
    }, delay);
  }

  return {
    async start() {
      await connectWithRecovery();
    },

    stop() {
      isStopping = true;
      if (pendingTimer != null) {
        clearTimeout(pendingTimer);
        pendingTimer = null;
      }
      // Fire-and-forget stop
      stopA2ex().catch(() => {});
    },
  };
}
