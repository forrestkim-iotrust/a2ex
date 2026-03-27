#!/bin/bash
set -euo pipefail

echo "[a2ex] Initializing WAIaaS..."
waiaas init --auto-provision || true

echo "[a2ex] Starting WAIaaS daemon..."
waiaas start &
WAIAAS_PID=$!

# Wait for WAIaaS health (max 30s)
for i in $(seq 1 60); do
  if curl -sf http://localhost:3100/health > /dev/null 2>&1; then
    echo "[a2ex] WAIaaS healthy (PID=$WAIAAS_PID)"
    break
  fi
  [ "$i" -eq 60 ] && echo "[a2ex] WAIaaS health timeout — exiting" && exit 1
  sleep 0.5
done

# Export env vars for plugin
export A2EX_BINARY_PATH="/usr/local/bin/a2ex-mcp"
export A2EX_WAIAAS_BASE_URL="http://localhost:3100"

# WAIaaS health monitor (background)
(
  FAIL_COUNT=0
  while true; do
    sleep 10
    if ! curl -sf http://localhost:3100/health > /dev/null 2>&1; then
      FAIL_COUNT=$((FAIL_COUNT + 1))
      echo "[a2ex] WAIaaS health check failed ($FAIL_COUNT/3)"
      [ "$FAIL_COUNT" -ge 3 ] && echo "[a2ex] WAIaaS unrecoverable — killing container" && kill 1 && exit 1
    else
      FAIL_COUNT=0
    fi
  done
) &

# Re-onboard with real API key (overwrites build-time placeholder)
if [ -n "${OPENROUTER_API_KEY:-}" ]; then
  npx -y openclaw@latest onboard --non-interactive --accept-risk \
    --auth-choice openrouter-api-key --openrouter-api-key "$OPENROUTER_API_KEY" \
    --gateway-auth token --gateway-token "${OPENCLAW_GATEWAY_TOKEN:-default}" \
    --gateway-bind lan --flow quickstart --skip-health 2>/dev/null || true
  echo "[a2ex] OpenClaw re-onboarded with runtime API key"
fi

echo "[a2ex] Starting OpenClaw gateway on :18789..."
exec npx -y openclaw@latest gateway \
  --allow-unconfigured --bind lan \
  --token "${OPENCLAW_GATEWAY_TOKEN}"
