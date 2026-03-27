"use client";

import { motion } from "framer-motion";

interface Strategy {
  id: string;
  name: string;
  venues: string[];
  riskLevel: string;
  minFundUsd: string;
  performance: { "7d": number; sparkline: number[] };
}

const strategies: Strategy[] = [
  {
    id: "polymarket-sports-basic",
    name: "Sports Basic",
    venues: ["Polymarket"],
    riskLevel: "Low",
    minFundUsd: "10",
    performance: { "7d": 0.032, sparkline: [30, 45, 40, 55, 50, 60, 65, 70, 60, 75] },
  },
  {
    id: "polymarket-politics-momentum",
    name: "Politics Momentum",
    venues: ["Polymarket"],
    riskLevel: "Medium",
    minFundUsd: "10",
    performance: { "7d": 0.081, sparkline: [20, 55, 35, 80, 100, 70, 50, 30, 20, 55] },
  },
  {
    id: "hyperliquid-perps-aggressive",
    name: "Perps Aggressive",
    venues: ["Hyperliquid"],
    riskLevel: "High",
    minFundUsd: "50",
    performance: { "7d": 0.124, sparkline: [15, 60, 100, 40, 15, 85, 70, 30, 95, 45] },
  },
];

function Sparkline({ data }: { data: number[] }) {
  const max = Math.max(...data);
  return (
    <div className="flex items-end gap-[2px] h-12 pt-2">
      {data.map((v, i) => (
        <motion.div
          key={i}
          className="flex-1 bg-accent rounded-t-[2px] min-h-[4px] opacity-60 hover:opacity-100 transition-opacity"
          initial={{ height: 0 }}
          whileInView={{ height: `${(v / max) * 100}%` }}
          viewport={{ once: true }}
          transition={{ delay: i * 0.05, duration: 0.4, ease: "easeOut" }}
        />
      ))}
    </div>
  );
}

export default function StrategyComparison({ onSelect }: { onSelect?: (id: string) => void }) {
  return (
    <div className="grid grid-cols-1 md:grid-cols-3 gap-[2px] bg-border rounded-lg overflow-hidden">
      {strategies.map((s) => (
        <div key={s.id} className="bg-surface p-8 flex flex-col gap-4">
          <div className="text-xs text-text-muted uppercase tracking-wider">{s.venues[0]}</div>
          <div className="text-lg font-semibold">{s.name}</div>
          <div className="flex justify-between items-baseline">
            <span className="text-[13px] text-text-muted">Risk</span>
            <span className="font-mono text-sm font-medium">{s.riskLevel}</span>
          </div>
          <div className="flex justify-between items-baseline">
            <span className="text-[13px] text-text-muted">Min Fund</span>
            <span className="font-mono text-sm font-medium">${s.minFundUsd}</span>
          </div>
          <div className="flex justify-between items-baseline">
            <span className="text-[13px] text-text-muted">7d Return</span>
            <span className="font-mono text-sm font-medium text-success">
              +{(s.performance["7d"] * 100).toFixed(1)}%
            </span>
          </div>
          <Sparkline data={s.performance.sparkline} />
          <button
            onClick={() => onSelect?.(s.id)}
            className="mt-auto py-2.5 text-center bg-accent-subtle text-accent rounded-sm font-semibold text-sm border border-transparent hover:border-accent transition-all"
          >
            Select Strategy
          </button>
        </div>
      ))}
    </div>
  );
}
