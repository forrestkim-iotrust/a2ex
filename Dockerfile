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
FROM node:22-slim AS plugin-builder

RUN corepack enable && corepack prepare pnpm@9 --activate

WORKDIR /build
COPY plugin/package.json plugin/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY plugin/tsconfig.json plugin/openclaw.plugin.json ./
COPY plugin/src/ ./src/
RUN pnpm build

# ============================================================
# Stage 3: Runtime — node:20 + openclaw + a2ex components
# ============================================================
FROM node:22-slim AS runtime

# Install system deps + tini for process supervision
RUN apt-get update && apt-get install -y --no-install-recommends \
    tini curl git && \
    rm -rf /var/lib/apt/lists/*

# Install OpenClaw + WAIaaS CLI globally
RUN npm install -g openclaw @waiaas/cli

# Create openclaw user (matching base image convention)
RUN useradd -m -s /bin/bash openclaw

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

# Onboard OpenClaw (creates base config — API key injected at runtime in entrypoint)
RUN npx -y openclaw@latest onboard --non-interactive --accept-risk \
  --auth-choice openrouter-api-key --openrouter-api-key "build-placeholder" \
  --gateway-auth token --gateway-token "build-placeholder" \
  --gateway-bind lan --flow quickstart --skip-health || true

# Install plugin into OpenClaw
RUN npx -y openclaw@latest plugins install --link /opt/a2ex-plugin || true

# Patch OpenClaw config via CLI (the canonical way)
RUN npx -y openclaw@latest config set agents.defaults.model.primary "openrouter/x-ai/grok-4.1-fast" && \
    npx -y openclaw@latest config set plugins.allow '["openclaw-plugin-a2ex"]' --strict-json && \
    npx -y openclaw@latest config set tools.allow '["group:plugins"]' --strict-json && \
    npx -y openclaw@latest config set gateway.http.endpoints.responses.enabled true --strict-json && \
    npx -y openclaw@latest config set gateway.http.endpoints.chatCompletions.enabled true --strict-json && \
    npx -y openclaw@latest config set gateway.controlUi.dangerouslyAllowHostHeaderOriginFallback true --strict-json && \
    echo "Config patched via CLI"

# Remove identity + stale settings.js (security — each deploy gets fresh identity)
RUN rm -rf ~/.openclaw/identity ~/.openclaw/settings.js ~/.openclaw/settings.json 2>/dev/null || true

# Copy entrypoint
COPY --chown=openclaw:openclaw docker/entrypoint.sh /entrypoint.sh
USER root
RUN chmod +x /entrypoint.sh
USER openclaw

EXPOSE 3100 18789

ENTRYPOINT ["tini", "--", "/entrypoint.sh"]
