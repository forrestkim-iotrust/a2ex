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

# Re-onboard with runtime API key
if [ -n "${OPENROUTER_API_KEY:-}" ]; then
  openclaw onboard --non-interactive --accept-risk \
    --auth-choice openrouter-api-key --openrouter-api-key "$OPENROUTER_API_KEY" \
    --gateway-auth token --gateway-token "${OPENCLAW_GATEWAY_TOKEN:-default}" \
    --gateway-bind lan --flow quickstart --skip-health 2>/dev/null || true
  echo "[a2ex] OpenClaw re-onboarded with runtime API key"
fi

echo "[a2ex] Starting OpenClaw gateway on :18789..."
exec openclaw gateway \
  --allow-unconfigured --bind lan \
  --token "${OPENCLAW_GATEWAY_TOKEN}"
