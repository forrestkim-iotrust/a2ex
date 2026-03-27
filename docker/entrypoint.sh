#!/bin/bash
set -euo pipefail

echo "[a2ex] Starting WAIaaS..."
waiaas init --non-interactive \
  --master-password "${WAIAAS_MASTER_PASSWORD:-auto-$(hostname)}" \
  --network "${WAIAAS_NETWORK:-arbitrum-mainnet}" || true

waiaas serve --port 3100 &
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

# WAIaaS health monitor (background) — exit container if WAIaaS dies
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

echo "[a2ex] Starting OpenClaw gateway on :18789..."
exec openclaw gateway \
  --allow-unconfigured --bind lan \
  --token "${OPENCLAW_GATEWAY_TOKEN}"
