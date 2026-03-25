#!/bin/bash
set -e

PLUGIN_DIR="${HOME}/.openclaw/extensions/openclaw-plugin-a2ex"
CONFIG_FILE="${HOME}/.openclaw/openclaw.json"
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

echo "[a2ex] Plugin files installed to $PLUGIN_DIR"

# Add plugin to config — triggers gateway auto-restart via config watcher
if [ -f "$CONFIG_FILE" ]; then
  echo "[a2ex] Updating config to enable plugin..."
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
  " 2>/dev/null && echo "[a2ex] Config updated. Gateway will auto-restart to load plugin." \
    || echo "[a2ex] Config update failed. Manual restart may be needed."
else
  echo "[a2ex] No config file found at $CONFIG_FILE. Manual restart may be needed."
fi
