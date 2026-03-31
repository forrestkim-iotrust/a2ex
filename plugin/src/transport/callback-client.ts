/**
 * Callback client — plugin → landing server HTTP communication.
 *
 * Reads CALLBACK_URL, CALLBACK_TOKEN, DEPLOYMENT_ID from environment.
 * These are injected by the Akash SDL at deploy time.
 *
 * Flow:
 *   Plugin (Akash container)  →  POST /api/agent/callback  →  Landing (Vercel)
 *                             ←  GET  /api/agent/callback  ←  (command polling)
 */

const MAX_RETRIES = 3;
const RETRY_DELAY_MS = 2000;

export interface CallbackConfig {
  callbackUrl: string;
  callbackToken: string;
  deploymentId: string;
}

export interface SecretsResponse {
  openrouterApiKey: string;
  waiaasPassword: string;
  gatewayToken: string;
  backupKey: string;
}

export interface CallbackClient {
  /** Send heartbeat with phase (bootstrap, ready, trading) */
  heartbeat(phase: string): Promise<void>;
  /** Report a trade event */
  reportTrade(trade: { venue: string; action: string; amountUsd: number; pnlUsd: number }): Promise<void>;
  /** Send a message from agent to user */
  sendMessage(content: string): Promise<void>;
  /** Poll for pending user commands, returns array of command strings */
  pollCommands(): Promise<string[]>;
  /** Fetch secrets (API keys, passwords, backup key) from landing server */
  fetchSecrets(): Promise<SecretsResponse | null>;
  /** Report encrypted WAIaaS backup data */
  reportBackup(encryptedData: string): Promise<void>;
  /** Report USDC wallet balance */
  reportBalance(usdcBalance: string): Promise<void>;
  /** Whether callback is configured (env vars present) */
  readonly enabled: boolean;
}

/**
 * Create a callback client from environment variables.
 * Returns a no-op client if env vars are missing (local dev without Akash).
 */
export function createCallbackClient(): CallbackClient {
  const callbackUrl = process.env.CALLBACK_URL;
  const callbackToken = process.env.CALLBACK_TOKEN;
  const deploymentId = process.env.DEPLOYMENT_ID;

  if (!callbackUrl || !callbackToken || !deploymentId) {
    return createNoopClient();
  }

  const config: CallbackConfig = { callbackUrl, callbackToken, deploymentId };
  return createHttpClient(config);
}

function createNoopClient(): CallbackClient {
  return {
    enabled: false,
    async heartbeat() {},
    async reportTrade() {},
    async sendMessage() {},
    async pollCommands() { return []; },
    async fetchSecrets() { return null; },
    async reportBackup() {},
    async reportBalance() {},
  };
}

function createHttpClient(config: CallbackConfig): CallbackClient {
  const { callbackUrl, callbackToken, deploymentId } = config;

  async function post(type: string, payload: Record<string, unknown>): Promise<any> {
    const body = JSON.stringify({ deploymentId, type, ...payload });

    for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
      try {
        const res = await fetch(callbackUrl, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${callbackToken}`,
          },
          body,
          signal: AbortSignal.timeout(10000),
        });

        if (res.status === 429) {
          // Rate limited — wait and retry
          await sleep(RETRY_DELAY_MS * (attempt + 1));
          continue;
        }

        if (!res.ok) {
          const text = await res.text().catch(() => "");
          console.error(`[callback] POST ${type} failed: ${res.status} ${text}`);
          return null;
        }

        return res.json().catch(() => ({}));
      } catch (err: any) {
        if (attempt < MAX_RETRIES) {
          console.warn(`[callback] POST ${type} attempt ${attempt + 1} failed: ${err.message}. Retrying...`);
          await sleep(RETRY_DELAY_MS * (attempt + 1));
        } else {
          console.error(`[callback] POST ${type} failed after ${MAX_RETRIES + 1} attempts: ${err.message}`);
        }
      }
    }
    return null;
  }

  return {
    enabled: true,

    async heartbeat(phase: string) {
      await post("heartbeat", { phase });
    },

    async reportTrade(trade) {
      await post("trade", trade);
    },

    async sendMessage(content: string) {
      await post("message", { content });
    },

    async pollCommands(): Promise<string[]> {
      try {
        const url = `${callbackUrl}?deploymentId=${deploymentId}`;
        const res = await fetch(url, {
          headers: { Authorization: `Bearer ${callbackToken}` },
          signal: AbortSignal.timeout(10000),
        });
        if (!res.ok) return [];
        const data = await res.json();
        return (data.commands ?? []).map((c: any) => c.content);
      } catch {
        return [];
      }
    },

    async fetchSecrets(): Promise<SecretsResponse | null> {
      try {
        const url = `${callbackUrl}?deploymentId=${deploymentId}&type=secrets`;
        const res = await fetch(url, {
          headers: { Authorization: `Bearer ${callbackToken}` },
          signal: AbortSignal.timeout(10000),
        });
        if (!res.ok) return null;
        return res.json();
      } catch {
        return null;
      }
    },

    async reportBackup(encryptedData: string) {
      await post("backup", { encryptedData });
    },

    async reportBalance(usdcBalance: string) {
      await post("balance_update", { usdcBalance });
    },
  };
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
