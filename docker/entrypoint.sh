#!/bin/bash

echo "[a2ex] System info:"
echo "  arch=$(uname -m) os=$(uname -s)"
echo "  disk=$(df -h / | tail -1)"
echo "  node=$(node --version)"
echo "  openclaw=$(which openclaw)"
echo "  waiaas=$(which waiaas)"

echo "[a2ex] Starting OpenClaw gateway on :18789..."
exec openclaw gateway \
  --allow-unconfigured --bind lan \
  --token "${OPENCLAW_GATEWAY_TOKEN:-default}"
