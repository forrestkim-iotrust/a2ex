// Tool names — prefixed with plugin id per OpenClaw convention
export const TOOL_SYSTEM_HEALTH = "a2ex_system_health";
export const TOOL_BOOTSTRAP = "a2ex_bootstrap";

// Dynamic MCP tool prefix
export const A2EX_TOOL_PREFIX = "a2ex_";

// WAIaaS runtime tool names
export const TOOL_WAIAAS_GET_BALANCE = "waiaas_get_balance";
export const TOOL_WAIAAS_GET_ADDRESS = "waiaas_get_address";
export const TOOL_WAIAAS_CALL_CONTRACT = "waiaas_call_contract";
export const TOOL_WAIAAS_SEND_TOKEN = "waiaas_send_token";
export const TOOL_WAIAAS_GET_TRANSACTION = "waiaas_get_transaction";
export const TOOL_WAIAAS_SIGN_MESSAGE = "waiaas_sign_message";
export const TOOL_WAIAAS_LIST_TRANSACTIONS = "waiaas_list_transactions";

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
  "a2ex_skills_load_bundle",
  "a2ex_skills_reload_bundle",
  "a2ex_skills_generate_proposal_packet",
  "a2ex_skills_stop_session",
  "a2ex_skills_clear_stop",
  "a2ex_onboarding_bootstrap_install",
  "a2ex_onboarding_apply_action",
  "a2ex_readiness_evaluate_route",
  "a2ex_readiness_apply_action",
  "a2ex_strategy_selection_materialize",
  "a2ex_strategy_selection_apply_override",
  "a2ex_strategy_selection_approve",
  "a2ex_strategy_selection_reopen",
  "a2ex_runtime_stop",
  "a2ex_runtime_pause",
  "a2ex_runtime_clear_stop",
  "a2ex_venue_prepare_bridge",
  "a2ex_venue_trade_polymarket",
  "a2ex_venue_trade_hyperliquid",
  "a2ex_venue_query_positions",
  "a2ex_venue_bridge_status",
  "a2ex_venue_derive_api_key",
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
