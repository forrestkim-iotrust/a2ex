#!/bin/bash
set -e

PLUGIN_DIR="${HOME}/.openclaw/extensions/openclaw-plugin-a2ex"
mkdir -p "$PLUGIN_DIR"
cd "$PLUGIN_DIR"

echo "[a2ex] Downloading openclaw-plugin-a2ex..."

# Download the tarball
npm pack openclaw-plugin-a2ex@latest 2>/dev/null

# Find the downloaded tgz file
TGZ=$(ls openclaw-plugin-a2ex-*.tgz 2>/dev/null | head -1)
if [ -z "$TGZ" ]; then
  echo "[a2ex] npm pack failed. Trying npm install instead..."
  npm install openclaw-plugin-a2ex@latest --prefix "$PLUGIN_DIR" --production 2>/dev/null
  if [ -d "$PLUGIN_DIR/node_modules/openclaw-plugin-a2ex" ]; then
    cp -r "$PLUGIN_DIR/node_modules/openclaw-plugin-a2ex/"* "$PLUGIN_DIR/"
  fi
fi

# Extract if tgz exists
if [ -n "$TGZ" ]; then
  tar xzf "$TGZ" --strip-components=1
  rm -f "$TGZ"
fi

# Install production dependencies
if [ -f package.json ]; then
  npm install --production --ignore-scripts 2>/dev/null || true
fi

echo "[a2ex] Plugin installed to $PLUGIN_DIR"

# Restart gateway to load the new plugin
echo "[a2ex] Restarting gateway to load plugin..."
GATEWAY_PID=$(pgrep -f "openclaw.*gateway" 2>/dev/null | head -1)
if [ -n "$GATEWAY_PID" ]; then
  kill -HUP "$GATEWAY_PID" 2>/dev/null || kill "$GATEWAY_PID" 2>/dev/null || true
  echo "[a2ex] Gateway restart signal sent (PID $GATEWAY_PID)"
else
  # Try openclaw CLI restart
  openclaw gateway restart 2>/dev/null || true
  echo "[a2ex] Gateway restart attempted via CLI"
fi

echo "[a2ex] Done. Plugin will be available after gateway restarts."
