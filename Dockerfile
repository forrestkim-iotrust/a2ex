# ============================================================
# Stage 1: Build Rust binary (a2ex-mcp)
# ============================================================
FROM rust:1.91-slim-bookworm AS rust-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev gcc && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY daemon/Cargo.toml daemon/Cargo.lock ./
COPY daemon/crates/ ./crates/

RUN cargo build --release -p a2ex-mcp

# ============================================================
# Stage 2: Build TypeScript plugin
# ============================================================
FROM node:20-slim AS plugin-builder

RUN corepack enable && corepack prepare pnpm@9 --activate

WORKDIR /build
COPY plugin/package.json plugin/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY plugin/tsconfig.json plugin/openclaw.plugin.json ./
COPY plugin/src/ ./src/
RUN pnpm build

# ============================================================
# Stage 3: Runtime — openclaw base + a2ex components
# ============================================================
FROM ghcr.io/forrestkim-iotrust/openclaw-base:latest AS runtime

USER root

# Install tini for process supervision
RUN apt-get update && apt-get install -y --no-install-recommends \
    tini curl && \
    rm -rf /var/lib/apt/lists/*

# Install WAIaaS CLI globally
RUN npm install -g @waiaas/cli

# Copy Rust binary
COPY --from=rust-builder /build/target/release/a2ex-mcp /usr/local/bin/a2ex-mcp
RUN chmod +x /usr/local/bin/a2ex-mcp

# Copy and install plugin
COPY --from=plugin-builder /build/dist/ /opt/a2ex-plugin/dist/
COPY --from=plugin-builder /build/package.json /opt/a2ex-plugin/
COPY --from=plugin-builder /build/openclaw.plugin.json /opt/a2ex-plugin/
COPY --from=plugin-builder /build/node_modules/ /opt/a2ex-plugin/node_modules/

# Switch to openclaw user for plugin install + config
USER openclaw

# Install plugin into OpenClaw
RUN npx -y openclaw@latest plugins install --link /opt/a2ex-plugin || true

# Patch OpenClaw config
RUN node -e " \
  const fs = require('fs'); \
  const home = require('os').homedir(); \
  const cfgPath = home + '/.openclaw/settings.json'; \
  let cfg = {}; \
  try { cfg = JSON.parse(fs.readFileSync(cfgPath, 'utf8')); } catch {} \
  cfg.agents = cfg.agents || {}; \
  cfg.agents.defaults = cfg.agents.defaults || {}; \
  cfg.agents.defaults.model = { primary: 'openrouter/minimax/minimax-m2.7' }; \
  cfg.plugins = { allow: ['openclaw-plugin-a2ex'] }; \
  cfg.tools = { allow: ['group:plugins'] }; \
  cfg.gateway = cfg.gateway || {}; \
  cfg.gateway.http = { endpoints: { responses: { enabled: true }, chatCompletions: { enabled: true } } }; \
  cfg.gateway.controlUi = { allowedOrigins: ['*'] }; \
  fs.mkdirSync(require('path').dirname(cfgPath), { recursive: true }); \
  fs.writeFileSync(cfgPath, JSON.stringify(cfg, null, 2)); \
  console.log('Config patched:', cfgPath); \
"

# Remove identity (security — each deploy gets fresh identity)
RUN rm -rf ~/.openclaw/identity 2>/dev/null || true

# Copy entrypoint
COPY --chown=openclaw:openclaw docker/entrypoint.sh /entrypoint.sh
USER root
RUN chmod +x /entrypoint.sh
USER openclaw

EXPOSE 3100 18789

ENTRYPOINT ["tini", "--", "/entrypoint.sh"]
