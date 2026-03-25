---
name: a2ex
description: Autonomous AI trading agent — Polymarket, Hyperliquid, cross-chain bridges. Install and run the a2ex OpenClaw plugin.
openclaw:
  install:
    - kind: node
      package: "openclaw-plugin-a2ex@latest"
---

# a2ex — Autonomous Trading Agent

You are now equipped with the a2ex autonomous trading skill. This skill enables you to trade on Polymarket, Hyperliquid, and bridge funds across chains — all non-custodial.

## First-Time Setup

Run these steps once to set up the trading infrastructure:

### Step 1: Install the plugin

```bash
openclaw plugins install openclaw-plugin-a2ex
```

This installs the a2ex plugin from npm. It registers 22+ trading tools with your agent.

### Step 2: Bootstrap wallets

Call the bootstrap tool to create the wallet infrastructure:

```
a2ex.bootstrap({ masterPassword: "<ask the user>", bundleUrl: "https://a2ex.vercel.app/bundle.json" })
```

Ask the user for a master password before calling bootstrap. This creates:
- **Vault wallet** — cold storage, approval-based, for large fund movements
- **Hot wallet** — instant, $50 limit, for fast trading

### Step 3: Fund the vault

After bootstrap, tell the user:
- The vault address (from bootstrap result)
- To send **USDC + ETH on Arbitrum** to that address
- USDC for trading, ETH for gas fees

Poll `waiaas.get_balance` to check when funded.

### Step 4: Start trading

Once funded, the strategy from the bundle will guide trading:
- Bridge funds to target chains via Across
- Place orders on Polymarket (prediction markets)
- Place orders on Hyperliquid (perpetual futures)

## Available Tools

After bootstrap completes, you have access to:

| Tool | Purpose |
|------|---------|
| `a2ex.system_health` | Check system status |
| `a2ex.bootstrap` | Initialize wallets (idempotent, safe to re-call) |
| `waiaas.get_balance` | Check wallet balances |
| `waiaas.get_address` | Get wallet addresses |
| `waiaas.call_contract` | Sign + submit contract transactions |
| `waiaas.send_token` | Send tokens |
| `a2ex.venue.trade_polymarket` | Trade on Polymarket |
| `a2ex.venue.trade_hyperliquid` | Trade on Hyperliquid |
| `a2ex.venue.prepare_bridge` | Prepare cross-chain bridge |
| `a2ex.venue.bridge_status` | Check bridge status |
| `a2ex.venue.derive_api_key` | Derive venue API credentials |
| `a2ex.runtime.stop` | Stop all trading |

## Safety Rules

- **Per-trade cap:** $10 USDC (demo stage)
- **Non-custodial:** 2-wallet model, keys never leave the machine
- **CONTRACT_WHITELIST:** Only approved contracts can be called
- **Kill switch:** Call `a2ex.runtime.stop` to halt all trading immediately

## Monitoring

Ask the user if they want status updates. Use `a2ex.system_health` to check:
- WAIaaS subprocess health
- a2ex engine health
- Open positions
- Recent trades
