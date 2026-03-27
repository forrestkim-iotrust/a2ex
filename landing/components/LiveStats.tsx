"use client";

import { useEffect, useState } from "react";
import { motion } from "framer-motion";

interface Stats {
  totalAgents: number;
  totalAumUsd: string;
  totalVolume: string;
  totalPnlUsd: string;
}

function AnimatedNumber({ value, prefix = "", suffix = "" }: { value: number; prefix?: string; suffix?: string }) {
  const [displayed, setDisplayed] = useState(0);

  useEffect(() => {
    const duration = 800;
    const start = Date.now();
    const from = displayed;
    const tick = () => {
      const elapsed = Date.now() - start;
      const progress = Math.min(elapsed / duration, 1);
      const eased = 1 - Math.pow(1 - progress, 3); // ease-out cubic
      setDisplayed(Math.round(from + (value - from) * eased));
      if (progress < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }, [value]);

  return (
    <span className="font-mono text-[32px] font-semibold text-accent tabular-nums">
      {prefix}{displayed.toLocaleString()}{suffix}
    </span>
  );
}

export default function LiveStats() {
  const [stats, setStats] = useState<Stats | null>(null);

  useEffect(() => {
    fetch("/api/stats")
      .then((r) => r.json())
      .then(setStats)
      .catch(() => {});
  }, []);

  const agents = stats?.totalAgents ?? 0;
  const aum = parseFloat(stats?.totalAumUsd ?? "0");
  const volume = parseFloat(stats?.totalVolume ?? "0");

  return (
    <div className="flex gap-12 border-t border-border pt-8 mt-12">
      <div>
        <AnimatedNumber value={agents} />
        <p className="text-[13px] text-text-muted mt-1">Agents Running</p>
      </div>
      <div>
        <AnimatedNumber value={Math.round(aum)} prefix="$" />
        <p className="text-[13px] text-text-muted mt-1">Total AUM</p>
      </div>
      <div>
        <AnimatedNumber value={Math.round(volume)} prefix="$" />
        <p className="text-[13px] text-text-muted mt-1">Volume Today</p>
      </div>
    </div>
  );
}
