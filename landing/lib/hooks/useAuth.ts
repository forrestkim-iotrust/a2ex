"use client";

import { useCallback, useState } from "react";
import { useAccount, useWalletClient } from "wagmi";
import { SiweMessage } from "siwe";

const BACKUP_MESSAGE = "a2ex backup key\n\nSigning creates your encrypted backup key.\nNo transaction will be sent.";

export function useAuth() {
  const { address, chainId } = useAccount();
  const { data: walletClient } = useWalletClient();
  const [isAuthenticating, setIsAuthenticating] = useState(false);

  const authenticate = useCallback(async (): Promise<boolean> => {
    if (!address || !walletClient) return false;
    setIsAuthenticating(true);

    try {
      // Step 1: Get nonce
      const nonceRes = await fetch("/api/auth/siwe");
      if (!nonceRes.ok) return false;
      const { nonce } = await nonceRes.json();

      // Step 2: Create & sign SIWE message
      // Uses walletClient.signMessage directly (no wagmi transport timeout)
      const siweMessage = new SiweMessage({
        domain: window.location.hostname,
        address,
        statement: "Sign in to A2EX",
        uri: window.location.origin,
        version: "1",
        chainId: chainId ?? 1,
        nonce,
        issuedAt: new Date().toISOString(),
      });

      const message = siweMessage.prepareMessage();
      const signature = await walletClient.signMessage({ message, account: address });

      // Step 3: Verify with server
      const verifyRes = await fetch("/api/auth/siwe", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ message, signature }),
      });
      if (!verifyRes.ok) return false;

      // Step 4: Sign backup key (optional — auth succeeds even if this fails)
      try {
        const backupSig = await walletClient.signMessage({ message: BACKUP_MESSAGE, account: address });
        const encoder = new TextEncoder();
        const hashBuffer = await crypto.subtle.digest("SHA-256", encoder.encode(backupSig));
        const backupKey = Array.from(new Uint8Array(hashBuffer))
          .map((b) => b.toString(16).padStart(2, "0"))
          .join("");
        await fetch("/api/auth/siwe", {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ backupKey }),
        });
      } catch (backupErr) {
        console.warn("[auth] Backup key signing skipped:", backupErr);
      }

      return true;
    } catch (err) {
      console.error("[auth] Failed:", err);
      return false;
    } finally {
      setIsAuthenticating(false);
    }
  }, [address, chainId, walletClient]);

  return { authenticate, isAuthenticating };
}
