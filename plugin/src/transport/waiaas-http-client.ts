import {
  WAIAAS_API_HEALTH,
  WAIAAS_API_WALLETS,
  WAIAAS_API_SESSIONS,
  WAIAAS_API_POLICIES,
  WAIAAS_API_BALANCE,
  WAIAAS_API_ADDRESS,
  WAIAAS_API_TRANSACTIONS_SEND,
  WAIAAS_API_SIGN_MESSAGE,
  WAIAAS_API_TRANSACTIONS,
} from "../constants.js";

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

export type WaiaasAuth =
  | { mode: "master"; masterPassword: string }
  | { mode: "session"; token: string };

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

export interface HealthResponse {
  status: string;
  version: string;
}

export interface CreateWalletRequest {
  name: string;
  chain: string;
  environment: string;
}

export interface CreateWalletResponse {
  id: string;
  publicKey: string;
}

export interface CreateSessionRequest {
  walletIds: string[];
}

export interface CreateSessionResponse {
  id: string;
  token: string;
}

export interface CreatePolicyRequest {
  walletId: string;
  type: string;
  rules: Record<string, unknown>;
  network?: string;
  priority?: number;
  enabled?: boolean;
}

export interface CreatePolicyResponse {
  id: string;
}

// --- Runtime tool request/response types ---

export interface BalanceResponse extends Record<string, unknown> {
  balance: string;
  network: string;
}

export interface AddressResponse extends Record<string, unknown> {
  address: string;
  network: string;
}

export interface SendTransactionRequest {
  walletId: string;
  network: string;
  to: string;
  value?: string;
  data?: string;
  [key: string]: unknown;
}

export interface SendTransactionResponse extends Record<string, unknown> {
  transactionId: string;
  hash?: string;
}

export interface SignMessageRequest {
  walletId: string;
  network: string;
  message: string;
  [key: string]: unknown;
}

export interface SignMessageResponse extends Record<string, unknown> {
  signature: string;
}

export interface TransactionResponse extends Record<string, unknown> {
  id: string;
  status: string;
  hash?: string;
}

export interface ListTransactionsResponse extends Record<string, unknown> {
  transactions: TransactionResponse[];
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

export class WaiaasApiError extends Error {
  readonly statusCode: number;
  readonly errorCode: string;
  readonly errorMessage: string;

  constructor(statusCode: number, errorCode: string, errorMessage: string) {
    super(`WAIaaS API error ${statusCode}: [${errorCode}] ${errorMessage}`);
    this.name = "WaiaasApiError";
    this.statusCode = statusCode;
    this.errorCode = errorCode;
    this.errorMessage = errorMessage;
  }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

export interface WaiaasClient {
  health(): Promise<HealthResponse>;
  createWallet(auth: WaiaasAuth, req: CreateWalletRequest): Promise<CreateWalletResponse>;
  createSession(auth: WaiaasAuth, req: CreateSessionRequest): Promise<CreateSessionResponse>;
  createPolicy(auth: WaiaasAuth, req: CreatePolicyRequest): Promise<CreatePolicyResponse>;

  // Runtime tool methods
  getBalance(auth: WaiaasAuth, params: { walletId: string; network: string }): Promise<BalanceResponse>;
  getAddress(auth: WaiaasAuth, params: { walletId: string; network: string }): Promise<AddressResponse>;
  sendTransaction(auth: WaiaasAuth, body: SendTransactionRequest): Promise<SendTransactionResponse>;
  signMessage(auth: WaiaasAuth, body: SignMessageRequest): Promise<SignMessageResponse>;
  getTransaction(auth: WaiaasAuth, id: string): Promise<TransactionResponse>;
  listTransactions(auth: WaiaasAuth, params: { walletId: string; network: string; status?: string; limit?: number }): Promise<ListTransactionsResponse>;
}

function buildHeaders(auth?: WaiaasAuth): Record<string, string> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };
  if (auth) {
    if (auth.mode === "master") {
      headers["X-Master-Password"] = auth.masterPassword;
    } else {
      headers["Authorization"] = `Bearer ${auth.token}`;
    }
  }
  return headers;
}

async function handleResponse<T>(res: Response): Promise<T> {
  if (!res.ok) {
    let errorCode = "UNKNOWN";
    let errorMessage = res.statusText;
    try {
      const body = (await res.json()) as Record<string, unknown>;
      if (typeof body.errorCode === "string") errorCode = body.errorCode;
      if (typeof body.error === "string") errorCode = body.error;
      if (typeof body.errorMessage === "string") errorMessage = body.errorMessage;
      if (typeof body.message === "string") errorMessage = body.message;
    } catch {
      // body not JSON — keep defaults
    }
    throw new WaiaasApiError(res.status, errorCode, errorMessage);
  }
  return (await res.json()) as T;
}

export function createWaiaasClient(baseUrl: string): WaiaasClient {
  return {
    async health(): Promise<HealthResponse> {
      const res = await fetch(`${baseUrl}${WAIAAS_API_HEALTH}`, {
        method: "GET",
        headers: { "Content-Type": "application/json" },
      });
      return handleResponse<HealthResponse>(res);
    },

    async createWallet(auth: WaiaasAuth, req: CreateWalletRequest): Promise<CreateWalletResponse> {
      const res = await fetch(`${baseUrl}${WAIAAS_API_WALLETS}`, {
        method: "POST",
        headers: buildHeaders(auth),
        body: JSON.stringify(req),
      });
      return handleResponse<CreateWalletResponse>(res);
    },

    async createSession(auth: WaiaasAuth, req: CreateSessionRequest): Promise<CreateSessionResponse> {
      const res = await fetch(`${baseUrl}${WAIAAS_API_SESSIONS}`, {
        method: "POST",
        headers: buildHeaders(auth),
        body: JSON.stringify(req),
      });
      return handleResponse<CreateSessionResponse>(res);
    },

    async createPolicy(auth: WaiaasAuth, req: CreatePolicyRequest): Promise<CreatePolicyResponse> {
      const res = await fetch(`${baseUrl}${WAIAAS_API_POLICIES}`, {
        method: "POST",
        headers: buildHeaders(auth),
        body: JSON.stringify(req),
      });
      return handleResponse<CreatePolicyResponse>(res);
    },

    async getBalance(auth: WaiaasAuth, params: { walletId: string; network: string }): Promise<BalanceResponse> {
      const qs = new URLSearchParams({ walletId: params.walletId, network: params.network });
      const res = await fetch(`${baseUrl}${WAIAAS_API_BALANCE}?${qs}`, {
        method: "GET",
        headers: buildHeaders(auth),
      });
      return handleResponse<BalanceResponse>(res);
    },

    async getAddress(auth: WaiaasAuth, params: { walletId: string; network: string }): Promise<AddressResponse> {
      const qs = new URLSearchParams({ walletId: params.walletId, network: params.network });
      const res = await fetch(`${baseUrl}${WAIAAS_API_ADDRESS}?${qs}`, {
        method: "GET",
        headers: buildHeaders(auth),
      });
      return handleResponse<AddressResponse>(res);
    },

    async sendTransaction(auth: WaiaasAuth, body: SendTransactionRequest): Promise<SendTransactionResponse> {
      const res = await fetch(`${baseUrl}${WAIAAS_API_TRANSACTIONS_SEND}`, {
        method: "POST",
        headers: buildHeaders(auth),
        body: JSON.stringify(body),
      });
      return handleResponse<SendTransactionResponse>(res);
    },

    async signMessage(auth: WaiaasAuth, body: SignMessageRequest): Promise<SignMessageResponse> {
      const res = await fetch(`${baseUrl}${WAIAAS_API_SIGN_MESSAGE}`, {
        method: "POST",
        headers: buildHeaders(auth),
        body: JSON.stringify(body),
      });
      return handleResponse<SignMessageResponse>(res);
    },

    async getTransaction(auth: WaiaasAuth, id: string): Promise<TransactionResponse> {
      const res = await fetch(`${baseUrl}${WAIAAS_API_TRANSACTIONS}/${encodeURIComponent(id)}`, {
        method: "GET",
        headers: buildHeaders(auth),
      });
      return handleResponse<TransactionResponse>(res);
    },

    async listTransactions(auth: WaiaasAuth, params: { walletId: string; network: string; status?: string; limit?: number }): Promise<ListTransactionsResponse> {
      const qs = new URLSearchParams({ walletId: params.walletId, network: params.network });
      if (params.status) qs.set("status", params.status);
      if (params.limit !== undefined) qs.set("limit", String(params.limit));
      const res = await fetch(`${baseUrl}${WAIAAS_API_TRANSACTIONS}?${qs}`, {
        method: "GET",
        headers: buildHeaders(auth),
      });
      return handleResponse<ListTransactionsResponse>(res);
    },
  };
}
