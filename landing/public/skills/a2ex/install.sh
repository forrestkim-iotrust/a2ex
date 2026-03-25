#!/bin/bash
set -e

PLUGIN_DIR="${HOME}/.openclaw/extensions/openclaw-plugin-a2ex"
CONFIG_FILE="${HOME}/.openclaw/openclaw.json"
BIN_DIR="${HOME}/.openclaw/bin"
RELEASE_URL="https://github.com/forrestkim-iotrust/a2ex/releases/latest/download"

mkdir -p "$PLUGIN_DIR" "$BIN_DIR"
cd "$PLUGIN_DIR"

# --- 1. Install plugin from npm ---
echo "[a2ex] Installing plugin..."

npm pack openclaw-plugin-a2ex@latest 2>/dev/null
TGZ=$(ls openclaw-plugin-a2ex-*.tgz 2>/dev/null | head -1)

if [ -z "$TGZ" ]; then
  npm install openclaw-plugin-a2ex@latest --prefix "$PLUGIN_DIR" --production 2>/dev/null
  if [ -d "$PLUGIN_DIR/node_modules/openclaw-plugin-a2ex" ]; then
    cp -r "$PLUGIN_DIR/node_modules/openclaw-plugin-a2ex/"* "$PLUGIN_DIR/"
  fi
else
  tar xzf "$TGZ" --strip-components=1
  rm -f "$TGZ"
fi

if [ -f package.json ]; then
  npm install --production --ignore-scripts 2>/dev/null || true
fi
echo "[a2ex] Plugin installed."

# --- 2. Download a2ex-mcp binary ---
echo "[a2ex] Downloading a2ex-mcp binary..."

ARCH=$(uname -m)
OS=$(uname -s | tr '[:upper:]' '[:lower:]')

if [ "$OS" = "linux" ] && [ "$ARCH" = "x86_64" ]; then
  TARGET="x86_64-unknown-linux-gnu"
elif [ "$OS" = "darwin" ] && [ "$ARCH" = "arm64" ]; then
  TARGET="aarch64-apple-darwin"
elif [ "$OS" = "darwin" ] && [ "$ARCH" = "x86_64" ]; then
  TARGET="x86_64-apple-darwin"
else
  echo "[a2ex] WARNING: No prebuilt binary for $OS/$ARCH. a2ex-mcp will not be available."
  TARGET=""
fi

if [ -n "$TARGET" ]; then
  curl -sL "${RELEASE_URL}/a2ex-mcp-${TARGET}.tar.gz" | tar xzf - -C "$BIN_DIR"
  chmod +x "$BIN_DIR/a2ex-mcp"
  echo "[a2ex] Binary installed to $BIN_DIR/a2ex-mcp"
fi

# --- 3. Update config to enable plugin + set binary path ---
if [ -f "$CONFIG_FILE" ]; then
  echo "[a2ex] Updating config..."
  node -e "
    const fs = require('fs');
    const cfg = JSON.parse(fs.readFileSync('$CONFIG_FILE', 'utf8'));
    cfg.plugins = cfg.plugins || {};
    cfg.plugins.entries = cfg.plugins.entries || {};
    cfg.plugins.entries['openclaw-plugin-a2ex'] = { enabled: true };
    cfg.plugins.allow = cfg.plugins.allow || [];
    if (!cfg.plugins.allow.includes('openclaw-plugin-a2ex')) {
      cfg.plugins.allow.push('openclaw-plugin-a2ex');
    }
    cfg.tools = cfg.tools || {};
    cfg.tools.allow = cfg.tools.allow || [];
    if (!cfg.tools.allow.includes('group:plugins')) {
      cfg.tools.allow.push('group:plugins');
    }
    fs.writeFileSync('$CONFIG_FILE', JSON.stringify(cfg, null, 2));
  " 2>/dev/null && echo "[a2ex] Config updated. Gateway will auto-restart." \
    || echo "[a2ex] Config update failed."
fi

echo "[a2ex] Installation complete. Plugin + binary ready."
