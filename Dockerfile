# ============================================================
# Stage 1: Build TypeScript plugin
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
# Stage 2: Runtime
# ============================================================
FROM node:22-slim AS runtime

ARG TARGETARCH
ARG DAEMON_VERSION=0.1.0

RUN apt-get update && apt-get install -y --no-install-recommends \
    tini curl && \
    rm -rf /var/lib/apt/lists/*

# Download pre-built a2ex-mcp binary from GitHub Releases (skip Rust build)
RUN ARCH_MAP="amd64:x86_64-unknown-linux-gnu arm64:aarch64-unknown-linux-gnu"; \
    TARGET=$(echo "$ARCH_MAP" | tr ' ' '\n' | grep "^${TARGETARCH}:" | cut -d: -f2); \
    echo "Downloading a2ex-mcp for $TARGET (arch=$TARGETARCH)"; \
    curl -fsSL "https://github.com/forrestkim-iotrust/a2ex/releases/download/daemon-v${DAEMON_VERSION}/a2ex-mcp-${TARGET}.tar.gz" \
      | tar xz -C /usr/local/bin/ && \
    chmod +x /usr/local/bin/a2ex-mcp && \
    a2ex-mcp --version || echo "binary installed"

RUN npm install -g openclaw@2026.3.24 @waiaas/cli && \
    which openclaw && openclaw --version && \
    which waiaas && waiaas --version

RUN useradd -m -s /bin/bash openclaw

COPY --from=plugin-builder /build/dist/ /opt/a2ex-plugin/dist/
COPY --from=plugin-builder /build/package.json /opt/a2ex-plugin/
COPY --from=plugin-builder /build/openclaw.plugin.json /opt/a2ex-plugin/
COPY --from=plugin-builder /build/node_modules/ /opt/a2ex-plugin/node_modules/

USER openclaw
RUN openclaw onboard --non-interactive --accept-risk \
      --auth-choice openrouter-api-key --openrouter-api-key "build-placeholder" \
      --gateway-auth token --gateway-token "build-placeholder" \
      --gateway-bind lan --flow quickstart --skip-health || true && \
    openclaw plugins install --link /opt/a2ex-plugin || true && \
    openclaw config set agents.defaults.model.primary "openrouter/x-ai/grok-4.1-fast" && \
    openclaw config set plugins.allow '["openclaw-plugin-a2ex"]' --strict-json && \
    openclaw config set tools.allow '["group:plugins"]' --strict-json && \
    openclaw config set gateway.http.endpoints.responses.enabled true --strict-json && \
    openclaw config set gateway.http.endpoints.chatCompletions.enabled true --strict-json && \
    openclaw config set gateway.controlUi.dangerouslyAllowHostHeaderOriginFallback true --strict-json && \
    rm -rf ~/.openclaw/identity ~/.openclaw/settings.js ~/.openclaw/settings.json \
           ~/.npm ~/.cache /tmp/* 2>/dev/null || true

COPY --chown=openclaw:openclaw docker/entrypoint.sh /entrypoint.sh
USER root
RUN chmod +x /entrypoint.sh
USER openclaw

EXPOSE 3100 18789
ENTRYPOINT ["/entrypoint.sh"]
