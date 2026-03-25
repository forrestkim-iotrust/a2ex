// Tool names — prefixed with plugin id per OpenClaw convention
export const TOOL_SYSTEM_HEALTH = "a2ex.system_health";
export const TOOL_BOOTSTRAP = "a2ex.bootstrap";

// Dynamic MCP tool prefix
export const A2EX_TOOL_PREFIX = "a2ex.";

// WAIaaS runtime tool names
export const TOOL_WAIAAS_GET_BALANCE = "waiaas.get_balance";
export const TOOL_WAIAAS_GET_ADDRESS = "waiaas.get_address";
export const TOOL_WAIAAS_CALL_CONTRACT = "waiaas.call_contract";
export const TOOL_WAIAAS_SEND_TOKEN = "waiaas.send_token";
export const TOOL_WAIAAS_GET_TRANSACTION = "waiaas.get_transaction";
export const TOOL_WAIAAS_SIGN_MESSAGE = "waiaas.sign_message";
export const TOOL_WAIAAS_LIST_TRANSACTIONS = "waiaas.list_transactions";

export const STATIC_PLUGIN_TOOL_NAMES = [
  TOOL_SYSTEM_HEALTH,
  TOOL_BOOTSTRAP,
  TOOL_WAIAAS_GET_BALANCE,
  TOOL_WAIAAS_GET_ADDRESS,
  TOOL_WAIAAS_CALL_CONTRACT,
  TOOL_WAIAAS_SEND_TOKEN,
  TOOL_WAIAAS_GET_TRANSACTION,
  TOOL_WAIAAS_SIGN_MESSAGE,
  TOOL_WAIAAS_LIST_TRANSACTIONS,
] as const;

export const KNOWN_DYNAMIC_A2EX_TOOL_NAMES = [
  "a2ex.skills.load_bundle",
  "a2ex.skills.reload_bundle",
  "a2ex.skills.generate_proposal_packet",
  "a2ex.skills.stop_session",
  "a2ex.skills.clear_stop",
  "a2ex.onboarding.bootstrap_install",
  "a2ex.onboarding.apply_action",
  "a2ex.readiness.evaluate_route",
  "a2ex.readiness.apply_action",
  "a2ex.strategy_selection.materialize",
  "a2ex.strategy_selection.apply_override",
  "a2ex.strategy_selection.approve",
  "a2ex.strategy_selection.reopen",
  "a2ex.runtime.stop",
  "a2ex.runtime.pause",
  "a2ex.runtime.clear_stop",
  "a2ex.venue.prepare_bridge",
  "a2ex.venue.trade_polymarket",
  "a2ex.venue.trade_hyperliquid",
  "a2ex.venue.query_positions",
  "a2ex.venue.bridge_status",
  "a2ex.venue.derive_api_key",
] as const;

// State file paths (relative to stateDir)
export const STATE_SUBDIR = "a2ex";
export const STATE_FILENAME = "a2ex-state.json";

// Default ports
export const DEFAULT_WAIAAS_PORT = 3100;
export const DEFAULT_A2EX_PORT = 0; // 0 = stdio mode
export const DEFAULT_WAIAAS_NETWORK = "arbitrum-mainnet";

// Health check interval
export const WAIAAS_HEALTHCHECK_INTERVAL_MS = 30_000;

// WAIaaS subprocess lifecycle
export const WAIAAS_HEALTHCHECK_TIMEOUT_MS = 30_000;
export const WAIAAS_HEALTHCHECK_POLL_MS = 500;
export const WAIAAS_DEFAULT_DATA_DIR = "waiaas-data";

// WAIaaS healthcheck recovery
export const WAIAAS_HEALTHCHECK_MAX_FAILURES = 3;

// A2EX close-event backoff recovery
export const A2EX_BACKOFF_INITIAL_MS = 1_000;
export const A2EX_BACKOFF_MAX_MS = 30_000;
export const A2EX_BACKOFF_MULTIPLIER = 2;

// WAIaaS API paths
export const WAIAAS_API_HEALTH = "/health";
export const WAIAAS_API_WALLETS = "/v1/wallets";
export const WAIAAS_API_SESSIONS = "/v1/sessions";
export const WAIAAS_API_POLICIES = "/v1/policies";
export const WAIAAS_API_BALANCE = "/v1/wallet/balance";
export const WAIAAS_API_ADDRESS = "/v1/wallet/address";
export const WAIAAS_API_TRANSACTIONS_SEND = "/v1/transactions/send";
export const WAIAAS_API_SIGN_MESSAGE = "/v1/transactions/sign-message";
export const WAIAAS_API_TRANSACTIONS = "/v1/transactions";
