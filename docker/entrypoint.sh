#!/bin/bash

echo "[a2ex] Initializing WAIaaS..."
waiaas init --auto-provision || true

echo "[a2ex] Starting WAIaaS daemon..."
waiaas start &

# Wait for WAIaaS health (max 30s)
for i in $(seq 1 60); do
  curl -sf http://localhost:3100/health > /dev/null 2>&1 && echo "[a2ex] WAIaaS healthy" && break
  [ "$i" -eq 60 ] && echo "[a2ex] WAIaaS timeout"
  sleep 0.5
done

export A2EX_BINARY_PATH="/usr/local/bin/a2ex-mcp"
export A2EX_WAIAAS_BASE_URL="http://localhost:3100"

# Inject runtime API key via paste-token (does NOT reset config like onboard does)
if [ -n "${OPENROUTER_API_KEY:-}" ]; then
  mkdir -p ~/.openclaw/agents/main/agent
  cat > ~/.openclaw/agents/main/agent/auth-profiles.json <<EOF
{
  "openrouter:default": {
    "provider": "openrouter",
    "token": "${OPENROUTER_API_KEY}",
    "createdAt": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  }
}
EOF
  echo "[a2ex] API key injected via auth-profiles.json"
fi

# Set gateway token at runtime (both auth and remote)
if [ -n "${OPENCLAW_GATEWAY_TOKEN:-}" ]; then
  openclaw config set gateway.auth.token "$OPENCLAW_GATEWAY_TOKEN" 2>/dev/null || true
  openclaw config set gateway.remote.token "$OPENCLAW_GATEWAY_TOKEN" 2>/dev/null || true
fi

echo "[a2ex] Starting OpenClaw gateway on :18789..."
exec openclaw gateway \
  --allow-unconfigured --bind lan \
  --token "${OPENCLAW_GATEWAY_TOKEN}"
