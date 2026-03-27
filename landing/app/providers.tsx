"use client";

import { WagmiProvider, createConfig, http, mock } from "wagmi";
import { arbitrum } from "wagmi/chains";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RainbowKitProvider, darkTheme, getDefaultConfig } from "@rainbow-me/rainbowkit";
import { privateKeyToAddress } from "viem/accounts";
import "@rainbow-me/rainbowkit/styles.css";
import { type ReactNode, useState, useEffect, useRef } from "react";
import type { Config } from "wagmi";
import type { Address } from "viem";

const TEST_KEY = process.env.NEXT_PUBLIC_TEST_PRIVATE_KEY;
const queryClient = new QueryClient();

function buildConfig(): Config {
  if (TEST_KEY) {
    const address = privateKeyToAddress(TEST_KEY as `0x${string}`);
    return createConfig({
      chains: [arbitrum],
      connectors: [
        mock({
          accounts: [address as Address],
          features: { defaultConnected: true, reconnect: true },
        }),
      ],
      transports: { [arbitrum.id]: http() },
    });
  }

  return getDefaultConfig({
    appName: "a2ex",
    projectId: process.env.NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID || "demo",
    chains: [arbitrum],
    ssr: true,
  });
}

export function Providers({ children }: { children: ReactNode }) {
  const [mounted, setMounted] = useState(false);
  const configRef = useRef<Config | null>(null);

  useEffect(() => {
    if (!configRef.current) {
      configRef.current = buildConfig();
    }
    setMounted(true);
  }, []);

  if (!mounted || !configRef.current) {
    return null;
  }

  return (
    <WagmiProvider config={configRef.current}>
      <QueryClientProvider client={queryClient}>
        <RainbowKitProvider
          theme={darkTheme({
            accentColor: "#f0a030",
            accentColorForeground: "#0c0c0e",
            borderRadius: "small",
          })}
        >
          {children}
        </RainbowKitProvider>
      </QueryClientProvider>
    </WagmiProvider>
  );
}
