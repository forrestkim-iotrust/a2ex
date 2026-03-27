import { useEffect, useRef } from "react";

export interface Trade {
  id: string;
}

export function useFirstTradeConfetti(trades: Trade[]) {
  const initialIdsRef = useRef<Set<string> | null>(null);
  const firedRef = useRef(false);

  useEffect(() => {
    // Record the initial set of trade IDs on first render
    if (initialIdsRef.current === null) {
      initialIdsRef.current = new Set(trades.map((t) => t.id));
      return;
    }

    if (firedRef.current) return;

    const hasNewTrade = trades.some((t) => !initialIdsRef.current!.has(t.id));
    if (hasNewTrade) {
      firedRef.current = true;
      import("canvas-confetti").then((mod) => {
        const confetti = mod.default;
        confetti({
          particleCount: 120,
          spread: 80,
          origin: { y: 0.6 },
        });
      });
    }
  }, [trades]);
}
