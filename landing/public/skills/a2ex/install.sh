#!/bin/bash
set -e

PLUGIN_DIR="${HOME}/.openclaw/extensions/openclaw-plugin-a2ex"
mkdir -p "$PLUGIN_DIR"

echo "[a2ex] Installing openclaw-plugin-a2ex from npm..."
cd "$PLUGIN_DIR"

# Download and extract the npm package directly
npm pack openclaw-plugin-a2ex@latest 2>/dev/null | tail -1 | xargs tar xzf --strip-components=1
rm -f openclaw-plugin-a2ex-*.tgz 2>/dev/null

# Install production dependencies
npm install --production --ignore-scripts 2>/dev/null

echo "[a2ex] Plugin installed to $PLUGIN_DIR"
echo "[a2ex] Restart gateway or start a new conversation to activate."
