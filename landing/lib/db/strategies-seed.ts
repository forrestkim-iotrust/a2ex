import { strategies } from "./schema";

export const strategySeedData = [
  {
    id: "polymarket-sports-basic",
    name: "Sports Basic",
    description: "Conservative prediction market strategy focused on high-probability sports outcomes.",
    venues: ["polymarket"],
    minFundUsd: "10",
    riskLevel: "low",
    performance: { "7d": 0.032, "30d": 0.089, sparkline: [30, 45, 40, 55, 50, 60, 65, 70, 60, 75] },
    active: true,
  },
  {
    id: "polymarket-politics-momentum",
    name: "Politics Momentum",
    description: "Medium-risk strategy trading political prediction markets based on momentum signals.",
    venues: ["polymarket"],
    minFundUsd: "10",
    riskLevel: "medium",
    performance: { "7d": 0.081, "30d": 0.142, sparkline: [20, 55, 35, 80, 100, 70, 50, 30, 20, 55] },
    active: true,
  },
  {
    id: "hyperliquid-perps-aggressive",
    name: "Perps Aggressive",
    description: "High-risk perpetual futures strategy on Hyperliquid with leverage.",
    venues: ["hyperliquid"],
    minFundUsd: "50",
    riskLevel: "high",
    performance: { "7d": 0.124, "30d": 0.067, sparkline: [15, 60, 100, 40, 15, 85, 70, 30, 95, 45] },
    active: true,
  },
];
