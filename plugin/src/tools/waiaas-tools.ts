import {
  TOOL_WAIAAS_GET_BALANCE,
  TOOL_WAIAAS_GET_ADDRESS,
  TOOL_WAIAAS_CALL_CONTRACT,
  TOOL_WAIAAS_SEND_TOKEN,
  TOOL_WAIAAS_GET_TRANSACTION,
  TOOL_WAIAAS_SIGN_MESSAGE,
  TOOL_WAIAAS_LIST_TRANSACTIONS,
  DEFAULT_WAIAAS_PORT,
} from "../constants.js";
import { readState, type A2exPluginState } from "../state/plugin-state.js";
import {
  createWaiaasClient,
  WaiaasApiError,
  type WaiaasAuth,
} from "../transport/waiaas-http-client.js";
import type { AnyAgentTool } from "../types/openclaw-plugin.js";

// ---------------------------------------------------------------------------
// MCP content envelope helpers (same pattern as bootstrap.ts)
// ---------------------------------------------------------------------------

function wrap(result: Record<string, unknown>) {
  return { content: [{ type: "text", text: JSON.stringify(result) }] };
}

function wrapError(message: string) {
  return {
    content: [{ type: "text", text: JSON.stringify({ error: message }) }],
    isError: true,
  };
}

// ---------------------------------------------------------------------------
// Shared state + auth helper
// ---------------------------------------------------------------------------

interface StateAndAuth {
  state: A2exPluginState;
  auth: WaiaasAuth;
  baseUrl: string;
}

type AuthScope = "vault" | "hot";

/**
 * Read persisted state, validate that vaultSessionToken exists,
 * and return auth + baseUrl ready for client calls.
 */
async function readStateAndAuth(
  getStateDir: () => string | null,
  scope: AuthScope,
): Promise<StateAndAuth> {
  const stateDir = getStateDir();
  if (stateDir == null) {
    throw new Error("Not bootstrapped — stateDir unavailable");
  }

  const state = await readState(stateDir);
  if (state == null) {
    throw new Error("Not bootstrapped — no state file found");
  }

  const token =
    scope === "hot" ? state.hotSessionToken : state.vaultSessionToken;

  if (!token) {
    throw new Error(
      `${scope}SessionToken not found — run bootstrap first`,
    );
  }

  const port = state.waiaasPort ?? DEFAULT_WAIAAS_PORT;
  return {
    state,
    auth: { mode: "session", token },
    baseUrl: `http://localhost:${port}`,
  };
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

export function createWaiaasTools(
  getStateDir: () => string | null,
): AnyAgentTool[] {
  // ----- get_balance -----
  const getBalance: AnyAgentTool = {
    name: TOOL_WAIAAS_GET_BALANCE,
    description:
      "Get the token balance for a wallet on a specific network.",
    parameters: {
      type: "object",
      properties: {
        walletId: { type: "string", description: "Wallet ID to query" },
        network: { type: "string", description: "Network identifier (e.g. arbitrum-mainnet)" },
      },
      required: ["walletId", "network"],
    },
    async execute(_toolCallId: string, params: Record<string, unknown>) {
      try {
        const { auth, baseUrl } = await readStateAndAuth(getStateDir, "vault");
        const client = createWaiaasClient(baseUrl);
        const result = await client.getBalance(auth, {
          walletId: params.walletId as string,
          network: params.network as string,
        });
        return wrap(result);
      } catch (err) {
        return wrapError(formatError(err));
      }
    },
  };

  // ----- get_address -----
  const getAddress: AnyAgentTool = {
    name: TOOL_WAIAAS_GET_ADDRESS,
    description:
      "Get the wallet address on a specific network.",
    parameters: {
      type: "object",
      properties: {
        walletId: { type: "string", description: "Wallet ID to query" },
        network: { type: "string", description: "Network identifier (e.g. arbitrum-mainnet)" },
      },
      required: ["walletId", "network"],
    },
    async execute(_toolCallId: string, params: Record<string, unknown>) {
      try {
        const { auth, baseUrl } = await readStateAndAuth(getStateDir, "vault");
        const client = createWaiaasClient(baseUrl);
        const result = await client.getAddress(auth, {
          walletId: params.walletId as string,
          network: params.network as string,
        });
        return wrap(result);
      } catch (err) {
        return wrapError(formatError(err));
      }
    },
  };

  // ----- call_contract -----
  const callContract: AnyAgentTool = {
    name: TOOL_WAIAAS_CALL_CONTRACT,
    description:
      "Execute a smart contract call. Sends a CONTRACT_CALL transaction.",
    parameters: {
      type: "object",
      properties: {
        walletId: { type: "string", description: "Wallet ID to send from" },
        network: { type: "string", description: "Network identifier" },
        to: { type: "string", description: "Contract address" },
        calldata: { type: "string", description: "ABI-encoded calldata (hex)" },
        value: { type: "string", description: "Optional ETH value in wei" },
      },
      required: ["walletId", "network", "to", "calldata"],
    },
    async execute(_toolCallId: string, params: Record<string, unknown>) {
      try {
        const { auth, baseUrl } = await readStateAndAuth(getStateDir, "vault");
        const client = createWaiaasClient(baseUrl);
        const result = await client.sendTransaction(auth, {
          type: "CONTRACT_CALL",
          walletId: params.walletId as string,
          network: params.network as string,
          to: params.to as string,
          calldata: params.calldata as string,
          ...(params.value != null ? { value: params.value as string } : {}),
        });
        return wrap(result);
      } catch (err) {
        return wrapError(formatError(err));
      }
    },
  };

  // ----- send_token -----
  const sendToken: AnyAgentTool = {
    name: TOOL_WAIAAS_SEND_TOKEN,
    description:
      "Send a native token transfer to an address.",
    parameters: {
      type: "object",
      properties: {
        walletId: { type: "string", description: "Wallet ID to send from" },
        network: { type: "string", description: "Network identifier" },
        to: { type: "string", description: "Recipient address" },
        amount: { type: "string", description: "Amount in wei" },
      },
      required: ["walletId", "network", "to", "amount"],
    },
    async execute(_toolCallId: string, params: Record<string, unknown>) {
      try {
        const { auth, baseUrl } = await readStateAndAuth(getStateDir, "vault");
        const client = createWaiaasClient(baseUrl);
        const result = await client.sendTransaction(auth, {
          type: "TRANSFER",
          walletId: params.walletId as string,
          network: params.network as string,
          to: params.to as string,
          value: params.amount as string,
        });
        return wrap(result);
      } catch (err) {
        return wrapError(formatError(err));
      }
    },
  };

  // ----- get_transaction -----
  const getTransaction: AnyAgentTool = {
    name: TOOL_WAIAAS_GET_TRANSACTION,
    description:
      "Get details of a previously submitted transaction by its ID.",
    parameters: {
      type: "object",
      properties: {
        walletId: { type: "string", description: "Wallet ID (required for session auth scope)" },
        transactionId: { type: "string", description: "Transaction ID to look up" },
      },
      required: ["walletId", "transactionId"],
    },
    async execute(_toolCallId: string, params: Record<string, unknown>) {
      try {
        const { auth, baseUrl } = await readStateAndAuth(getStateDir, "vault");
        const client = createWaiaasClient(baseUrl);
        const result = await client.getTransaction(
          auth,
          params.transactionId as string,
        );
        return wrap(result);
      } catch (err) {
        return wrapError(formatError(err));
      }
    },
  };

  // ----- sign_message -----
  const signMessage: AnyAgentTool = {
    name: TOOL_WAIAAS_SIGN_MESSAGE,
    description:
      "Sign an arbitrary message with a wallet's private key.",
    parameters: {
      type: "object",
      properties: {
        walletId: { type: "string", description: "Wallet ID to sign with" },
        network: { type: "string", description: "Network identifier" },
        message: { type: "string", description: "Message to sign" },
        signType: { type: "string", description: "Optional sign type (e.g. EIP-712)" },
        typedData: { type: "string", description: "Optional typed data JSON for EIP-712" },
      },
      required: ["walletId", "network", "message"],
    },
    async execute(_toolCallId: string, params: Record<string, unknown>) {
      try {
        const { auth, baseUrl, state } = await readStateAndAuth(getStateDir, "hot");
        if (!state.hotWalletId) {
          return wrapError("hotWalletId not found — run bootstrap first");
        }
        const client = createWaiaasClient(baseUrl);
        const result = await client.signMessage(auth, {
          walletId: params.walletId as string,
          network: params.network as string,
          message: params.message as string,
          ...(params.signType != null ? { signType: params.signType as string } : {}),
          ...(params.typedData != null ? { typedData: params.typedData as string } : {}),
        });
        return wrap(result);
      } catch (err) {
        return wrapError(formatError(err));
      }
    },
  };

  // ----- list_transactions -----
  const listTransactions: AnyAgentTool = {
    name: TOOL_WAIAAS_LIST_TRANSACTIONS,
    description:
      "List transactions for a wallet, optionally filtered by network, status, and limit.",
    parameters: {
      type: "object",
      properties: {
        walletId: { type: "string", description: "Wallet ID to query" },
        network: { type: "string", description: "Network identifier" },
        status: { type: "string", description: "Optional status filter (e.g. pending, confirmed)" },
        limit: { type: "number", description: "Optional max results" },
      },
      required: ["walletId", "network"],
    },
    async execute(_toolCallId: string, params: Record<string, unknown>) {
      try {
        const { auth, baseUrl } = await readStateAndAuth(getStateDir, "vault");
        const client = createWaiaasClient(baseUrl);
        const result = await client.listTransactions(auth, {
          walletId: params.walletId as string,
          network: params.network as string,
          ...(params.status != null ? { status: params.status as string } : {}),
          ...(params.limit != null ? { limit: params.limit as number } : {}),
        });
        return wrap(result);
      } catch (err) {
        return wrapError(formatError(err));
      }
    },
  };

  return [
    getBalance,
    getAddress,
    callContract,
    sendToken,
    getTransaction,
    signMessage,
    listTransactions,
  ];
}

// ---------------------------------------------------------------------------
// Error formatting
// ---------------------------------------------------------------------------

function formatError(err: unknown): string {
  if (err instanceof WaiaasApiError) {
    return `WAIaaS API error ${err.statusCode}: [${err.errorCode}] ${err.errorMessage}`;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return String(err);
}
