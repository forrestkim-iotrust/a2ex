import type { OpenClawPluginApi } from "./types/openclaw-plugin.js";
import { createToolFactory } from "./tools/factory.js";
import {
  KNOWN_DYNAMIC_A2EX_TOOL_NAMES,
  STATIC_PLUGIN_TOOL_NAMES,
  DEFAULT_WAIAAS_NETWORK,
  STATE_SUBDIR,
} from "./constants.js";
import {
  startA2exWithRecovery,
} from "./services/a2ex.service.js";
import { clearMcpCache } from "./tools/a2ex-dynamic.js";
import {
  isWaiaasRunning,
  stopWaiaas,
  startWaiaasHealthcheck,
} from "./services/waiaas.service.js";
import { readState, writeState } from "./state/plugin-state.js";
import type { A2exPluginState } from "./state/plugin-state.js";
import { createCallbackClient, type CallbackClient } from "./transport/callback-client.js";
import { join } from "node:path";
import { createHash } from "node:crypto";

// ---------------------------------------------------------------------------
// Credential forwarding — build env for a2ex subprocess
// ---------------------------------------------------------------------------

function resolveWaiaasDataDir(stateDir: string): string {
  return join(stateDir, STATE_SUBDIR, "waiaas-data");
}

/**
 * Build the environment variables to forward hot-wallet credentials
 * from plugin state to the a2ex subprocess.
 *
 * Security invariant: only hot-wallet credentials are forwarded.
 * `vaultSessionToken` and `masterPassword` MUST NEVER appear in the result.
 *
 * Returns `undefined` when no credential fields are present (the a2ex binary
 * will fall back to default mode with no venue adapters — see T01).
 */
export function buildA2exSubprocessEnv(
  state: A2exPluginState,
): Record<string, string> | undefined {
  const env: Record<string, string> = {};

  if (state.waiaasPort != null) {
    env.A2EX_WAIAAS_BASE_URL = `http://localhost:${state.waiaasPort}`;
  }

  if (state.hotWalletId != null && state.hotWalletId !== "") {
    env.A2EX_HOT_WALLET_ID = state.hotWalletId;
  }

  if (state.hotAddress != null && state.hotAddress !== "") {
    env.A2EX_HOT_WALLET_ADDRESS = state.hotAddress;
  }

  if (state.hotSessionToken != null && state.hotSessionToken !== "") {
    env.A2EX_HOT_SESSION_TOKEN = state.hotSessionToken;
  }

  // Network defaults to mainnet — always set when any credential is present
  env.A2EX_WAIAAS_NETWORK = DEFAULT_WAIAAS_NETWORK;

  // Return undefined when no meaningful credentials are present
  // (waiaasPort alone without tokens isn't useful)
  if (!env.A2EX_HOT_SESSION_TOKEN && !env.A2EX_HOT_WALLET_ID) {
    return undefined;
  }

  return env;
}

/**
 * Module-level captured stateDir.
 *
 * Set by the service's start() callback when OpenClaw boots the plugin.
 * The tool factory's getStateDir thunk closes over this variable, enabling
 * per-turn state reads without stateDir being on the tool context.
 */
let capturedStateDir: string | null = null;

/**
 * Module-level lifecycle flags and handles.
 *
 * isStopping — set true in stop() to prevent recovery loops during shutdown.
 * healthcheckHandle — WAIaaS periodic healthcheck with auto-restart.
 * a2exRecoveryHandle — a2ex close-event recovery with exponential backoff.
 */
let isStopping = false;
let healthcheckHandle: { stop: () => void } | null = null;
let a2exRecoveryHandle: { stop: () => void; start: () => Promise<void> } | null = null;
let callbackClient: CallbackClient | null = null;
let heartbeatInterval: ReturnType<typeof setInterval> | null = null;
let commandPollInterval: ReturnType<typeof setInterval> | null = null;
let backupKey: string | null = null;
let lastBackupHash: string | null = null;

/** Thunk for dependency injection into the tool factory. */
const getStateDir = (): string | null => capturedStateDir;

/**
 * Reset module state. Used by test harness to ensure isolation between tests.
 * Not part of the public plugin API.
 */
export function __resetForTesting(): void {
  capturedStateDir = null;
  isStopping = false;
  // Stop active handles to prevent recovery timers from leaking across tests
  healthcheckHandle?.stop();
  healthcheckHandle = null;
  a2exRecoveryHandle?.stop();
  a2exRecoveryHandle = null;
  if (heartbeatInterval) { clearInterval(heartbeatInterval); heartbeatInterval = null; }
  if (commandPollInterval) { clearInterval(commandPollInterval); commandPollInterval = null; }
  callbackClient = null;
  backupKey = null;
  lastBackupHash = null;
  clearMcpCache();
}

/**
 * Encrypt WAIaaS data directory for backup using AES-256-GCM.
 * Returns base64-encoded encrypted data, or null on failure.
 */
async function encryptAndUploadBackup(stateDir: string): Promise<boolean> {
  if (!callbackClient?.enabled || !backupKey) return false;

  try {
    const { readFileSync, readdirSync, statSync } = await import("node:fs");
    const { join: pathJoin } = await import("node:path");
    const crypto = await import("node:crypto");

    const waiaasDir = resolveWaiaasDataDir(stateDir);

    // Collect WAIaaS data files into a JSON bundle
    const bundle: Record<string, string> = {};
    function collectFiles(dir: string, prefix: string) {
      try {
        for (const entry of readdirSync(dir)) {
          const full = pathJoin(dir, entry);
          const key = prefix ? `${prefix}/${entry}` : entry;
          const stat = statSync(full);
          if (stat.isDirectory()) {
            collectFiles(full, key);
          } else if (stat.size < 1_000_000) {
            bundle[key] = readFileSync(full).toString("base64");
          }
        }
      } catch { /* dir may not exist yet */ }
    }
    collectFiles(waiaasDir, "");

    // Also include plugin state
    const state = await readState(stateDir);
    if (state) {
      bundle["__plugin_state"] = JSON.stringify(state);
    }

    const plaintext = JSON.stringify(bundle);

    // Check if data changed since last backup
    const hash = createHash("sha256").update(plaintext).digest("hex");
    if (hash === lastBackupHash) return true; // no change

    // AES-256-GCM encrypt
    const keyBuf = Buffer.from(backupKey, "hex").subarray(0, 32);
    const iv = crypto.randomBytes(12);
    const cipher = crypto.createCipheriv("aes-256-gcm", keyBuf, iv);
    const encrypted = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
    const tag = cipher.getAuthTag();

    // Format: iv(12) + tag(16) + ciphertext, base64 encoded
    const combined = Buffer.concat([iv, tag, encrypted]);
    const encryptedData = combined.toString("base64");

    await callbackClient.reportBackup(encryptedData);
    lastBackupHash = hash;
    console.log(`[backup] Uploaded encrypted backup (${Math.round(encryptedData.length / 1024)}KB)`);
    return true;
  } catch (err: any) {
    console.warn(`[backup] Failed: ${err.message}`);
    return false;
  }
}

/**
 * Plugin entry point. Called by OpenClaw when the plugin is loaded.
 *
 * Wires:
 * 1. A service whose start() captures stateDir, wires WAIaaS healthcheck
 *    if WAIaaS is running, and auto-connects to a2ex with crash recovery
 *    if state indicates a binaryPath is configured.
 * 2. A tool factory that resolves tools per-turn based on persisted state.
 */
export default function register(api: OpenClawPluginApi): void {
  api.registerService({
    id: "a2ex",
    async start(ctx) {
      capturedStateDir = ctx.stateDir;
      isStopping = false;

      // Initialize callback client (reads CALLBACK_URL/TOKEN/DEPLOYMENT_ID env vars)
      callbackClient = createCallbackClient();
      if (callbackClient.enabled) {
        // Fetch secrets from landing server (API keys, passwords, backup key)
        const secrets = await callbackClient.fetchSecrets();
        if (secrets) {
          if (secrets.waiaasPassword) {
            process.env.WAIAAS_MASTER_PASSWORD = secrets.waiaasPassword;
          }
          if (secrets.openrouterApiKey) {
            process.env.OPENROUTER_API_KEY = secrets.openrouterApiKey;
          }
          if (secrets.backupKey) {
            backupKey = secrets.backupKey;
          }
          console.log("[callback] Secrets fetched from landing server");
        }

        // Send initial heartbeat immediately
        callbackClient.heartbeat("bootstrap").catch(() => {});

        // Periodic heartbeat every 30s + backup attempt
        heartbeatInterval = setInterval(async () => {
          if (!isStopping && callbackClient?.enabled) {
            callbackClient.heartbeat(a2exRecoveryHandle ? "trading" : "ready").catch(() => {});

            // Attempt backup on each heartbeat (skips if data unchanged)
            if (capturedStateDir) {
              encryptAndUploadBackup(capturedStateDir).catch(() => {});
            }
          }
        }, 30_000);

        // Poll for user commands every 5s
        commandPollInterval = setInterval(async () => {
          if (!isStopping && callbackClient?.enabled) {
            const commands = await callbackClient.pollCommands();
            for (const cmd of commands) {
              if (cmd === "SYSTEM:BACKUP_NOW") {
                console.log("[callback] Received BACKUP_NOW command");
                if (capturedStateDir) {
                  await encryptAndUploadBackup(capturedStateDir);
                }
              } else if (cmd === "SYSTEM:SHUTDOWN") {
                console.log("[callback] Received SHUTDOWN command");
                // Backup before shutdown
                if (capturedStateDir) {
                  await encryptAndUploadBackup(capturedStateDir);
                }
              } else if (commands.length > 0) {
                console.log(`[callback] Received command: ${cmd}`);
              }
            }
          }
        }, 5_000);
      }

      let state = await readState(ctx.stateDir);
      if (state != null && state.waiaasDataDir == null) {
        await writeState(ctx.stateDir, {
          ...state,
          waiaasDataDir: resolveWaiaasDataDir(ctx.stateDir),
        });
        state = {
          ...state,
          waiaasDataDir: resolveWaiaasDataDir(ctx.stateDir),
        };
      }

      // Wire WAIaaS healthcheck if state has a live waiaasPid
      if (state?.waiaasPid && isWaiaasRunning(state.waiaasPid)) {
        healthcheckHandle = startWaiaasHealthcheck({
          pid: state.waiaasPid,
          port: state.waiaasPort,
          restartOptions: state.waiaasDataDir
            ? {
                dataDir: state.waiaasDataDir,
                masterPassword: state.masterPassword ?? "a2ex-e2e-default-mp",
                port: state.waiaasPort,
              }
            : undefined,
          onRestart: async (newPid: number) => {
            // Update persisted state with the new WAIaaS PID
            const currentState = await readState(ctx.stateDir);
            if (currentState) {
              await writeState(ctx.stateDir, {
                ...currentState,
                waiaasPid: newPid,
              });
            }
          },
          onDegraded: async () => {
            const currentState = await readState(ctx.stateDir);
            if (currentState) {
              await writeState(ctx.stateDir, {
                ...currentState,
                phase: "bootstrapping",
                waiaasPid: undefined,
                a2exPid: undefined,
              });
            }
          },
        });
      }

      // Auto-connect to a2ex with crash recovery if state has a binaryPath
      if (state?.binaryPath) {
        const a2exEnv = buildA2exSubprocessEnv(state);
        const handle = startA2exWithRecovery({
          binaryPath: state.binaryPath,
          stateDir: ctx.stateDir,
          ...(a2exEnv != null ? { env: a2exEnv } : {}),
          onRestart: () => {
            // Recovery event — tools will be re-populated via setMcpCache
          },
        });
        a2exRecoveryHandle = handle;
        await handle.start();
      }
    },

    async stop() {
      isStopping = true;

      // Tear down callback intervals
      if (heartbeatInterval) { clearInterval(heartbeatInterval); heartbeatInterval = null; }
      if (commandPollInterval) { clearInterval(commandPollInterval); commandPollInterval = null; }
      callbackClient = null;

      // Tear down WAIaaS healthcheck interval
      healthcheckHandle?.stop();
      healthcheckHandle = null;

      // Tear down a2ex recovery loop (sets its own isStopping + calls stopA2ex)
      a2exRecoveryHandle?.stop();
      a2exRecoveryHandle = null;

      const state = capturedStateDir == null ? null : await readState(capturedStateDir);
      if (capturedStateDir != null && state != null) {
        if (state.waiaasPid) {
          stopWaiaas(state.waiaasPid);
        }
        await writeState(capturedStateDir, {
          ...state,
          phase: "bootstrapping",
          waiaasPid: undefined,
          a2exPid: undefined,
          waiaasDataDir: state.waiaasDataDir ?? resolveWaiaasDataDir(capturedStateDir),
        });
      }
    },
  });

  // Register tool factory with explicit names.
  // In OpenClaw 2026.3.13+, factory functions don't auto-extract names,
  // so we must provide them via opts.names for allowlist matching.
  api.registerTool(createToolFactory(getStateDir), {
    names: [...STATIC_PLUGIN_TOOL_NAMES, ...KNOWN_DYNAMIC_A2EX_TOOL_NAMES],
  });
}
