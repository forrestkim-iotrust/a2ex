#!/bin/bash

# ──────────────────────────────────────────────────────────
# Phase 0: Fetch secrets from landing server
# Secrets are NOT in the SDL (on-chain visible). They're stored
# in the DB and delivered via the authenticated callback channel.
# ──────────────────────────────────────────────────────────
fetch_secrets() {
  local url="${CALLBACK_URL}?deploymentId=${DEPLOYMENT_ID}&type=secrets"
  local result
  result=$(curl -sf --max-time 10 "$url" \
    -H "Authorization: Bearer ${CALLBACK_TOKEN}" 2>/dev/null)
  echo "$result"
}

if [ -n "${CALLBACK_URL:-}" ] && [ -n "${CALLBACK_TOKEN:-}" ] && [ -n "${DEPLOYMENT_ID:-}" ]; then
  echo "[a2ex] Fetching secrets from landing server..."

  SECRETS=""
  for attempt in 1 2 3 4 5; do
    SECRETS=$(fetch_secrets)
    if [ -n "$SECRETS" ] && echo "$SECRETS" | node -e "JSON.parse(require('fs').readFileSync('/dev/stdin','utf8'))" 2>/dev/null; then
      break
    fi
    echo "[a2ex] Secret fetch attempt $attempt failed, retrying in ${attempt}s..."
    sleep "$attempt"
  done

  if [ -n "$SECRETS" ]; then
    export WAIAAS_MASTER_PASSWORD=$(echo "$SECRETS" | node -e "process.stdout.write(JSON.parse(require('fs').readFileSync('/dev/stdin','utf8')).waiaasPassword||'')" 2>/dev/null)
    export OPENROUTER_API_KEY=$(echo "$SECRETS" | node -e "process.stdout.write(JSON.parse(require('fs').readFileSync('/dev/stdin','utf8')).openrouterApiKey||'')" 2>/dev/null)
    echo "[a2ex] Secrets loaded (waiaas=$([ -n "$WAIAAS_MASTER_PASSWORD" ] && echo 'yes' || echo 'no'), openrouter=$([ -n "$OPENROUTER_API_KEY" ] && echo 'yes' || echo 'no'))"
  else
    echo "[a2ex] WARNING: Failed to fetch secrets after 5 attempts"
  fi
else
  echo "[a2ex] No callback config — running with env vars (local dev mode)"
fi

# ──────────────────────────────────────────────────────────
# Phase 1: Start WAIaaS
# ──────────────────────────────────────────────────────────
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

# ──────────────────────────────────────────────────────────
# Phase 2: Configure OpenClaw
# ──────────────────────────────────────────────────────────
export A2EX_BINARY_PATH="/usr/local/bin/a2ex-mcp"
export A2EX_WAIAAS_BASE_URL="http://localhost:3100"

# Inject runtime API key via auth-profiles.json
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

# Set gateway token at runtime
if [ -n "${OPENCLAW_GATEWAY_TOKEN:-}" ]; then
  openclaw config set gateway.auth.token "$OPENCLAW_GATEWAY_TOKEN" 2>/dev/null || true
  openclaw config set gateway.remote.token "$OPENCLAW_GATEWAY_TOKEN" 2>/dev/null || true
fi

# ──────────────────────────────────────────────────────────
# Phase 3: Start OpenClaw gateway
# ──────────────────────────────────────────────────────────
echo "[a2ex] Starting OpenClaw gateway on :18789..."
exec openclaw gateway \
  --allow-unconfigured --bind lan \
  --token "${OPENCLAW_GATEWAY_TOKEN}"
