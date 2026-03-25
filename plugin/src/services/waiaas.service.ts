import { execFile, spawn, type ChildProcess } from "node:child_process";
import { promisify } from "node:util";
import {
  WAIAAS_HEALTHCHECK_TIMEOUT_MS,
  WAIAAS_HEALTHCHECK_POLL_MS,
  WAIAAS_HEALTHCHECK_INTERVAL_MS,
  WAIAAS_HEALTHCHECK_MAX_FAILURES,
  WAIAAS_API_HEALTH,
} from "../constants.js";

const execFileAsync = promisify(execFile);

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface StartWaiaasOptions {
  dataDir: string;
  masterPassword: string;
  port: number;
}

export interface StartWaiaasResult {
  pid: number;
  port: number;
}

export class WaiaasStartupError extends Error {
  readonly elapsedMs: number;

  constructor(message: string, elapsedMs: number) {
    super(message);
    this.name = "WaiaasStartupError";
    this.elapsedMs = elapsedMs;
  }
}

// ---------------------------------------------------------------------------
// Healthcheck poller
// ---------------------------------------------------------------------------

async function pollHealthcheck(
  baseUrl: string,
  timeoutMs: number = WAIAAS_HEALTHCHECK_TIMEOUT_MS,
  pollMs: number = WAIAAS_HEALTHCHECK_POLL_MS,
): Promise<void> {
  const url = `${baseUrl}${WAIAAS_API_HEALTH}`;
  const start = Date.now();

  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(url, { method: "GET" });
      if (res.ok) {
        const body = (await res.json()) as { status?: string };
        if (body.status === "ok") return;
      }
    } catch {
      // Connection refused / network error — keep polling
    }
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }

  throw new WaiaasStartupError(
    `WAIaaS healthcheck did not respond within ${timeoutMs}ms`,
    Date.now() - start,
  );
}

// ---------------------------------------------------------------------------
// Single health check (exported for testability)
// ---------------------------------------------------------------------------

/**
 * Perform a single health check against the WAIaaS server.
 * Returns true if the server responds with `{ status: "ok" }`, false otherwise.
 */
export async function checkHealth(baseUrl: string): Promise<boolean> {
  const url = `${baseUrl}${WAIAAS_API_HEALTH}`;
  try {
    const res = await fetch(url, { method: "GET" });
    if (res.ok) {
      const body = (await res.json()) as { status?: string };
      return body.status === "ok";
    }
    return false;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Start the WAIaaS subprocess.
 *
 * 1. Runs `npx waiaas init --data-dir <dataDir>` (idempotent — safe to re-run).
 * 2. Spawns `npx waiaas start --data-dir <dataDir>` with WAIAAS_MASTER_PASSWORD
 *    set as an env var (never passed as CLI arg or stored).
 * 3. Polls GET /health until `{ status: "ok" }` or timeout.
 *
 * @returns PID and port of the running subprocess.
 */
export async function startWaiaas(
  options: StartWaiaasOptions,
): Promise<StartWaiaasResult> {
  const { dataDir, masterPassword, port } = options;

  // (a) Init — idempotent (ignore "Already initialized" exit code)
  try {
    await execFileAsync("npx", ["-y", "@waiaas/cli", "init", "--data-dir", dataDir]);
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    if (!msg.includes("Already initialized")) throw err;
  }

  // (b) Spawn start — masterPassword only via env var
  const childEnv: Record<string, string> = {
    ...process.env as Record<string, string>,
    WAIAAS_MASTER_PASSWORD: masterPassword,
  };

  const child: ChildProcess = spawn(
    "npx",
    ["-y", "@waiaas/cli", "start", "--data-dir", dataDir],
    {
      env: childEnv,
      stdio: "ignore",
      detached: true,
    },
  );

  // Ensure the parent doesn't wait for the child
  child.unref();

  if (!child.pid) {
    throw new WaiaasStartupError("Failed to spawn WAIaaS process — no PID", 0);
  }

  // (c) Poll healthcheck
  const baseUrl = `http://localhost:${port}`;
  await pollHealthcheck(baseUrl);

  return { pid: child.pid, port };
}

/**
 * Check if a WAIaaS process is still running.
 * Uses `process.kill(pid, 0)` — sends no signal, just checks existence.
 */
export function isWaiaasRunning(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/**
 * Stop a WAIaaS process by sending SIGTERM.
 */
export function stopWaiaas(pid: number): void {
  try {
    process.kill(pid, "SIGTERM");
  } catch {
    // Process already exited — safe to ignore
  }
}

// ---------------------------------------------------------------------------
// Healthcheck loop with auto-restart
// ---------------------------------------------------------------------------

export interface StartWaiaasHealthcheckOptions {
  pid: number;
  port: number;
  /** StartWaiaasOptions needed to restart the process. */
  restartOptions?: StartWaiaasOptions;
  /** Called after a successful restart with the new PID. */
  onRestart?: (newPid: number) => void;
  /** Called when the process is unhealthy and restart is unavailable or fails. */
  onDegraded?: () => void;
  /** Override the check interval (ms). Defaults to WAIAAS_HEALTHCHECK_INTERVAL_MS. */
  intervalMs?: number;
  /** Override startWaiaas for testing. */
  _startFn?: (opts: StartWaiaasOptions) => Promise<StartWaiaasResult>;
  /** Override stopWaiaas for testing. */
  _stopFn?: (pid: number) => void;
  /** Override checkHealth for testing. */
  _checkFn?: (baseUrl: string) => Promise<boolean>;
}

/**
 * Start a periodic WAIaaS healthcheck that auto-restarts after
 * `WAIAAS_HEALTHCHECK_MAX_FAILURES` consecutive failures.
 *
 * Returns a handle with `stop()` to cancel the loop.
 */
export function startWaiaasHealthcheck(
  options: StartWaiaasHealthcheckOptions,
): { stop: () => void } {
  const {
    port,
    restartOptions,
    onRestart,
    onDegraded,
    intervalMs = WAIAAS_HEALTHCHECK_INTERVAL_MS,
    _startFn = startWaiaas,
    _stopFn = stopWaiaas,
    _checkFn = checkHealth,
  } = options;

  let currentPid = options.pid;
  let failCount = 0;
  let isStopping = false;
  let restarting = false;

  const baseUrl = `http://localhost:${port}`;

  const timer = setInterval(async () => {
    if (isStopping || restarting) return;

    const ok = await _checkFn(baseUrl);

    if (ok) {
      failCount = 0;
      return;
    }

    failCount++;

    if (failCount >= WAIAAS_HEALTHCHECK_MAX_FAILURES) {
      if (isStopping) return;

      if (!restartOptions) {
        onDegraded?.();
        return;
      }

      restarting = true;
      failCount = 0;

      try {
        _stopFn(currentPid);
        const result = await _startFn(restartOptions);
        currentPid = result.pid;
        onRestart?.(result.pid);
      } catch {
        // Restart failed — will retry on next cycle
        onDegraded?.();
      } finally {
        restarting = false;
      }
    }
  }, intervalMs);

  return {
    stop() {
      isStopping = true;
      clearInterval(timer);
    },
  };
}
