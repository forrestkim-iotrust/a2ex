"use client";

import { ConnectButton } from "@rainbow-me/rainbowkit";

export default function ConnectWalletButton() {
  return (
    <ConnectButton.Custom>
      {({ account, chain, openConnectModal, openChainModal, mounted }) => {
        const connected = mounted && account && chain;

        return (
          <div
            {...(!mounted && {
              "aria-hidden": true,
              style: { opacity: 0, pointerEvents: "none", userSelect: "none" },
            })}
          >
            {!connected ? (
              <button
                onClick={openConnectModal}
                className="inline-flex items-center gap-2 rounded-md bg-accent px-7 py-3.5 text-[15px] font-semibold text-bg transition-all hover:bg-accent-hover hover:-translate-y-0.5"
              >
                Connect Wallet & Deploy
              </button>
            ) : chain?.unsupported ? (
              <button
                onClick={openChainModal}
                className="inline-flex items-center gap-2 rounded-md border border-danger bg-danger/10 px-5 py-2.5 text-sm font-medium text-danger"
              >
                Switch to Arbitrum
              </button>
            ) : (
              <div className="flex items-center gap-3">
                <button
                  onClick={openChainModal}
                  className="flex items-center gap-1.5 rounded-md border border-border bg-surface px-3 py-2 text-sm transition hover:border-accent"
                >
                  {chain?.name}
                </button>
                <div className="rounded-md border border-border bg-surface px-4 py-2 font-mono text-sm tabular-nums">
                  {account.displayBalance} · {account.displayName}
                </div>
              </div>
            )}
          </div>
        );
      }}
    </ConnectButton.Custom>
  );
}
