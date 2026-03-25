# a2ex — Autonomous AI Trading Agent

> One line in OpenClaw. Fully autonomous trading across Polymarket, Hyperliquid, and more.

## Quick Start

Open your OpenClaw chat and type:

```
a2ex 플러그인 설치하고, 지갑 만들고, Polymarket에서 트레이딩 시작해줘
```

Or in English:

```
Install the a2ex plugin, set up my wallets, and start trading on Polymarket
```

That's it. a2ex installs itself, creates non-custodial wallets, bridges your funds, and starts executing trades.

## What is a2ex?

a2ex is an **OpenClaw plugin** that gives your AI agent the ability to trade autonomously across multiple venues:

- **Polymarket** — Prediction markets
- **Hyperliquid** — Perpetual futures DEX
- **Cross-chain bridges** — Via Across Protocol

### Key Features

- **One Line Setup** — Tell OpenClaw what you want, the AI handles everything
- **Non-Custodial** — 2-wallet model (Vault + Hot) with MPC signing. Your keys never leave your machine
- **Multi-Chain** — Arbitrum, Polygon, and cross-chain bridges
- **22+ Trading Tools** — Registered as MCP tools in your OpenClaw agent

## How It Works

```
You: "Install a2ex and trade on Polymarket"
  │
  ▼
┌─────────────────────────────────────────────┐
│  OpenClaw Docker Container                  │
│                                             │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │ OpenClaw │→│  a2ex    │→│  WAIaaS  │  │
│  │    AI    │  │  Plugin  │  │  Wallet  │  │
│  │          │  │ 22 Tools │  │ Signing  │  │
│  └──────────┘  └────┬─────┘  └──────────┘  │
│                     │                       │
└─────────────────────┼───────────────────────┘
                      │
        ┌─────────────┼─────────────┐
        ▼             ▼             ▼
  ┌──────────┐ ┌──────────┐ ┌──────────┐
  │Polymarket│ │Hyperliquid│ │  Across  │
  │  CLOB    │ │  Perps   │ │  Bridge  │
  └──────────┘ └──────────┘ └──────────┘
```

## Install

### Via OpenClaw (recommended)

Just tell your OpenClaw agent:
```
Install the a2ex trading plugin
```

### Via npm

```bash
openclaw plugin install openclaw-plugin-a2ex
```

### Via npm directly

```bash
npm install openclaw-plugin-a2ex
```

## Repository Structure

```
├── plugin/     TypeScript OpenClaw plugin (npm: openclaw-plugin-a2ex)
├── daemon/     Rust trading engine (a2ex-mcp, 22+ MCP tools)
└── landing/    Landing page (Next.js)
```

## Prerequisites

- **OpenClaw** running (any version with plugin support)
- **USDC + ETH** on Arbitrum (for trading)

## Safety

- Per-trade cap: $10 USDC (demo stage)
- 2-wallet model: Vault (approval-based) + Hot ($50 instant limit)
- CONTRACT_WHITELIST: Only approved contracts can be called
- Kill switch: Disable plugin in OpenClaw to stop all trading

## Early Access

a2ex is in early access. Known limitations:
- Supported venues: Polymarket, Hyperliquid
- WAIaaS session may expire after extended idle (auto-recovers)
- Requires OpenClaw with plugin support

## Development

```bash
# Plugin
cd plugin && pnpm install && pnpm build && pnpm test

# Daemon
cd daemon && cargo build --workspace && cargo test --workspace

# Landing
cd landing && pnpm install && pnpm dev
```

## License

MIT

## Built by [Iotrust](https://github.com/IotrustGitHub)
