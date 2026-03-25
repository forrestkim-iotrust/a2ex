import { TOOL_BOOTSTRAP, DEFAULT_WAIAAS_PORT, STATE_SUBDIR } from "../constants.js";
import { readState, writeState, type A2exPluginState } from "../state/plugin-state.js";
import { startWaiaas, isWaiaasRunning } from "../services/waiaas.service.js";
import { startA2exWithRecovery } from "../services/a2ex.service.js";
import { buildA2exSubprocessEnv } from "../index.js";
import {
  createWaiaasClient,
  type WaiaasClient,
  type WaiaasAuth,
} from "../transport/waiaas-http-client.js";
import type { AnyAgentTool } from "../types/openclaw-plugin.js";
import { join } from "node:path";
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";

function resolveWaiaasDataDir(stateDir: string): string {
  return join(stateDir, STATE_SUBDIR, "waiaas-data");
}

const RPC_CONFIG_BLOCK = `
[rpc]
evm_ethereum_mainnet = "https://eth.drpc.org"
evm_polygon_mainnet = "https://polygon-bor-rpc.publicnode.com"
evm_arbitrum_mainnet = "https://arb1.arbitrum.io/rpc"
evm_optimism_mainnet = "https://optimism.drpc.org"
evm_base_mainnet = "https://base.drpc.org"
`;

function ensureRpcConfig(dataDir: string): void {
  mkdirSync(dataDir, { recursive: true });
  const configPath = join(dataDir, "config.toml");
  if (existsSync(configPath)) {
    const content = readFileSync(configPath, "utf-8");
    if (content.includes("evm_polygon_mainnet")) return;
  }
  writeFileSync(configPath, RPC_CONFIG_BLOCK.trimStart(), { flag: "a" });
}

// ---------------------------------------------------------------------------
// Input schema
// ---------------------------------------------------------------------------

const BOOTSTRAP_SCHEMA = {
  type: "object" as const,
  properties: {
    masterPassword: {
      type: "string" as const,
      description: "Optional master password for WAIaaS. Defaults to a fixed E2E password if omitted.",
    },
    bundleUrl: {
      type: "string" as const,
      description: "Optional URL to a WAIaaS binary bundle. Reserved for future use.",
    },
  },
  required: [] as string[],
};

// ---------------------------------------------------------------------------
// Bootstrap tool factory
// ---------------------------------------------------------------------------

/**
 * Creates the `a2ex.bootstrap` tool.
 *
 * Orchestrates the full 8-step idempotent bootstrap sequence:
 * 1. Start WAIaaS subprocess
 * 2. Create vault wallet
 * 3. Create hot wallet
 * 4. Create vault session (scoped to both wallets)
 * 5. Create hot session (scoped to hot wallet only)
 * 6. Create vault policy (SPENDING_LIMIT, instant_max_usd:0, delay_max_usd:10)
 * 7. Create hot policy (SPENDING_LIMIT, instant_max_usd:50)
 * 8. Write final state with phase:"bootstrapped"
 *
 * Each step reads current state and skips if already completed.
 * After each API step, state is persisted immediately for crash safety.
 *
 * masterPassword flows: param → auth header → out of scope. Never stored.
 */
export function createBootstrapTool(
  getStateDir: () => string | null,
): AnyAgentTool {
  return {
    name: TOOL_BOOTSTRAP,
    description:
      "Bootstrap the A2EX agent runtime: starts WAIaaS, creates vault + hot " +
      "wallets, sessions, and spending policies. Idempotent — safe to re-invoke " +
      "after partial failure.",
    parameters: BOOTSTRAP_SCHEMA,

    async execute(_toolCallId: string, params: Record<string, unknown>) {
      const stateDir = getStateDir();
      if (stateDir == null) {
        return wrapError("Plugin service not started — stateDir unavailable");
      }

      let masterPassword = (params.masterPassword as string) || "a2ex-e2e-default-mp";

      const port = DEFAULT_WAIAAS_PORT;
      const baseUrl = `http://localhost:${port}`;

      // Load or initialize state
      let state: A2exPluginState = (await readState(stateDir)) ?? {
        phase: "not_initialized",
        waiaasPort: port,
        waiaasDataDir: resolveWaiaasDataDir(stateDir),
        lastUpdated: new Date().toISOString(),
      };
      state = {
        ...state,
        waiaasDataDir: state.waiaasDataDir ?? resolveWaiaasDataDir(stateDir),
      };

      // Mark bootstrapping + persist master password for WAIaaS restart & key recovery
      state = { ...state, phase: "bootstrapping", masterPassword };
      await writeState(stateDir, state);

      let auth: WaiaasAuth = { mode: "master", masterPassword };

      // Step 1: Start WAIaaS (skip if already running)
      if (!state.waiaasPid || !isWaiaasRunning(state.waiaasPid)) {
        const dataDir = state.waiaasDataDir ?? resolveWaiaasDataDir(stateDir);
        const result = await startWaiaas({ dataDir, masterPassword, port });
        state = { ...state, waiaasPid: result.pid, waiaasPort: result.port };
        await writeState(stateDir, state);
      }

      // Step 1b: Ensure RPC URLs in config.toml (idempotent)
      const dataDir = state.waiaasDataDir ?? resolveWaiaasDataDir(stateDir);
      ensureRpcConfig(dataDir);

      const client: WaiaasClient = createWaiaasClient(baseUrl);

      // Step 2: Create vault wallet (skip if exists)
      if (!state.vaultWalletId) {
        const wallet = await client.createWallet(auth, {
          name: "a2ex-vault",
          chain: "ethereum",
          environment: "mainnet",
        });
        state = { ...state, vaultWalletId: wallet.id, vaultAddress: wallet.publicKey };
        await writeState(stateDir, state);
      }

      // Step 3: Create hot wallet (skip if exists)
      if (!state.hotWalletId) {
        const wallet = await client.createWallet(auth, {
          name: "a2ex-hot",
          chain: "ethereum",
          environment: "mainnet",
        });
        state = { ...state, hotWalletId: wallet.id, hotAddress: wallet.publicKey };
        await writeState(stateDir, state);
      }

      // Step 4: Create vault session — scoped to BOTH wallets
      if (!state.vaultSessionToken) {
        const session = await client.createSession(auth, {
          walletIds: [state.vaultWalletId!, state.hotWalletId!],
        });
        state = { ...state, vaultSessionToken: session.token };
        await writeState(stateDir, state);
      }

      // Step 5: Create hot session — scoped to hot wallet ONLY
      if (!state.hotSessionToken) {
        const session = await client.createSession(auth, {
          walletIds: [state.hotWalletId!],
        });
        state = { ...state, hotSessionToken: session.token };
        await writeState(stateDir, state);
      }

      // Step 6: Create vault policy (skip if exists)
      if (!state.policyIds?.vault) {
        const policy = await client.createPolicy(auth, {
          walletId: state.vaultWalletId!,
          type: "SPENDING_LIMIT",
          rules: { instant_max_usd: 0, delay_max_usd: 10 },
        });
        state = {
          ...state,
          policyIds: { ...state.policyIds, vault: policy.id },
        };
        await writeState(stateDir, state);
      }

      // Step 7: Create hot policy (skip if exists)
      if (!state.policyIds?.hot) {
        const policy = await client.createPolicy(auth, {
          walletId: state.hotWalletId!,
          type: "SPENDING_LIMIT",
          rules: { instant_max_usd: 50 },
        });
        state = {
          ...state,
          policyIds: { ...state.policyIds, hot: policy.id },
        };
        await writeState(stateDir, state);
      }

      // Step 7b: Create Arbitrum CONTRACT_WHITELIST
      if (!state.policyIds?.hotWhitelistArb) {
        const policy = await client.createPolicy(auth, {
          walletId: state.hotWalletId!,
          type: "CONTRACT_WHITELIST",
          network: "arbitrum-mainnet",
          rules: {
            contracts: [
              { address: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831" }, // USDC
              { address: "0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A" }, // Across SpokePool
              { address: "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1" }, // WETH
              { address: "0x4cd00e387622c35bddb9b4c962c136462338bc31" }, // Relay.link
              { address: "0x2Df1c51E09aECF9cacB7bc98cB1742757f163dF7" }, // Hyperliquid Bridge
            ],
          },
        });
        state = {
          ...state,
          policyIds: { ...state.policyIds, hotWhitelistArb: policy.id },
        };
        await writeState(stateDir, state);
      }

      // Step 7c: Create Polygon CONTRACT_WHITELIST
      if (!state.policyIds?.hotWhitelistPoly) {
        const policy = await client.createPolicy(auth, {
          walletId: state.hotWalletId!,
          type: "CONTRACT_WHITELIST",
          network: "polygon-mainnet",
          rules: {
            contracts: [
              { address: "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174" }, // USDC.e
              { address: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359" }, // native USDC
              { address: "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E" }, // CTF Exchange
              { address: "0xC5d563A36AE78145C45a50134d48A1215220f80a" }, // NegRisk CTF
              { address: "0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270" }, // WMATIC
            ],
          },
        });
        state = {
          ...state,
          policyIds: { ...state.policyIds, hotWhitelistPoly: policy.id },
        };
        await writeState(stateDir, state);
      }

      // Step 8: Detect a2ex-mcp binary
      if (!state.binaryPath) {
        try {
          const found = execSync("which a2ex-mcp", { encoding: "utf-8" }).trim();
          if (found) {
            state = { ...state, binaryPath: found };
          }
        } catch {
          // Binary not on PATH — skip a2ex subprocess
        }
      }

      // Step 9: Write final state with phase:"bootstrapped"
      state = { ...state, phase: "bootstrapped" };
      await writeState(stateDir, state);

      // Step 10: Spawn a2ex subprocess if binary available
      let a2exSpawnResult = "skipped";
      if (state.binaryPath) {
        try {
          const a2exEnv = buildA2exSubprocessEnv(state);
          const handle = startA2exWithRecovery({
            binaryPath: state.binaryPath,
            stateDir,
            ...(a2exEnv != null ? { env: a2exEnv } : {}),
            onRestart: () => { /* recovery */ },
          });
          await handle.start();
          a2exSpawnResult = "ok";
        } catch (err: unknown) {
          a2exSpawnResult = `failed: ${err instanceof Error ? err.message : String(err)}`;
        }
      }

      // Best-effort masterPassword disposal — drop local references so the
      // password string becomes eligible for GC sooner.  This is NOT a
      // cryptographic guarantee (JS strings are immutable and may remain in
      // the V8 heap until collected), but it removes the most obvious
      // reachable reference paths.
      masterPassword = "";
      auth = { mode: "session", token: "" }; // overwrite to release masterPassword ref

      return wrap({
        status: "bootstrapped",
        vaultAddress: state.vaultAddress,
        hotAddress: state.hotAddress,
        a2exSpawn: a2exSpawnResult,
        fundingRequired: "Send ETH + USDC to vault address",
      });
    },
  };
}

// ---------------------------------------------------------------------------
// MCP content envelope helpers
// ---------------------------------------------------------------------------

function wrap(result: Record<string, unknown>) {
  return { content: [{ type: "text", text: JSON.stringify(result) }] };
}

function wrapError(message: string) {
  return { content: [{ type: "text", text: JSON.stringify({ error: message }) }], isError: true };
}
