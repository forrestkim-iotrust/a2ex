---
name: a2ex
description: On-chain execution engine for OpenClaw. Enables autonomous multi-venue operations.
---

# a2ex

On-chain execution engine. Run `/a2ex` to set up.

## Setup (run once)

### Step 1: Install the plugin

Download and run the installer:

```bash
curl -sL https://a2ex.vercel.app/skills/a2ex/install.sh | bash
```

This places the plugin in `~/.openclaw/extensions/openclaw-plugin-a2ex/`.

After installation, tell the user: "Plugin installed. Please start a new conversation so the plugin loads."

Do NOT restart the gateway. Do NOT kill any processes. Just tell the user to start a new conversation.

### Step 2: Bootstrap (in the new conversation)

After installation, call the bootstrap tool:

```
a2ex.bootstrap({ masterPassword: "<ask the user for a master password>", bundleUrl: "https://a2ex.vercel.app/bundle.json" })
```

Ask the user for a master password first. Bootstrap creates the required infrastructure.

### Step 3: Follow tool output

The bootstrap result will contain an address and instructions. Relay them to the user.
Then poll `waiaas.get_balance` until the user confirms.

### Step 4: Execute strategy

Once ready, load the strategy from the bundle and begin execution using the available tools.

## Available Tools (after bootstrap)

- `a2ex.system_health` — status check
- `a2ex.bootstrap` — initialize (idempotent)
- `waiaas.get_balance` — check balances
- `waiaas.call_contract` — sign and submit
- `a2ex.venue.*` — venue operations
- `a2ex.runtime.stop` — halt execution
