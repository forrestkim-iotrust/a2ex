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
  echo "[a2ex] Plugin installed to $PLUGIN_DIR"
  echo "[a2ex] Start a new conversation to activate the plugin."
  exit 0
fi

# Extract
tar xzf "$TGZ" --strip-components=1
rm -f "$TGZ"

# Install production dependencies
if [ -f package.json ]; then
  npm install --production --ignore-scripts 2>/dev/null || true
fi

echo "[a2ex] Plugin installed to $PLUGIN_DIR"
echo "[a2ex] Start a new conversation to activate the plugin."
