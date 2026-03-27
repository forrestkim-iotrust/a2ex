"use client";

import { WagmiProvider } from "wagmi";
import { arbitrum } from "wagmi/chains";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RainbowKitProvider, darkTheme, getDefaultConfig } from "@rainbow-me/rainbowkit";
import "@rainbow-me/rainbowkit/styles.css";
import { type ReactNode, useState, useEffect, useRef } from "react";
import type { Config } from "wagmi";

const queryClient = new QueryClient();

export function Providers({ children }: { children: ReactNode }) {
  const [mounted, setMounted] = useState(false);
  const configRef = useRef<Config | null>(null);

  useEffect(() => {
    // Create wagmi config only on client side to avoid SSR localStorage/indexedDB errors
    if (!configRef.current) {
      configRef.current = getDefaultConfig({
        appName: "a2ex",
        projectId: process.env.NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID || "demo",
        chains: [arbitrum],
        ssr: true,
      });
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
