import { pgTable, text, uuid, numeric, boolean, timestamp, jsonb } from "drizzle-orm/pg-core";
import { integer } from "drizzle-orm/pg-core";

export const deployments = pgTable("deployments", {
  id: uuid("id").primaryKey().defaultRandom(),
  userAddress: text("user_address").notNull(),
  akashDseq: text("akash_dseq"),
  strategyId: text("strategy_id").notNull(),
  config: jsonb("config").notNull(),
  status: text("status").notNull().default("pending"),
  hotAddress: text("hot_address"),
  createdAt: timestamp("created_at", { withTimezone: true }).defaultNow(),
  terminatedAt: timestamp("terminated_at", { withTimezone: true }),
});

export const trades = pgTable("trades", {
  id: uuid("id").primaryKey().defaultRandom(),
  deploymentId: uuid("deployment_id").references(() => deployments.id),
  venue: text("venue").notNull(),
  action: text("action").notNull(),
  amountUsd: numeric("amount_usd", { precision: 12, scale: 2 }),
  pnlUsd: numeric("pnl_usd", { precision: 12, scale: 2 }),
  ts: timestamp("ts", { withTimezone: true }).defaultNow(),
});

export const statsSnapshots = pgTable("stats_snapshots", {
  id: uuid("id").primaryKey().defaultRandom(),
  totalAgents: integer("total_agents"),
  totalAumUsd: numeric("total_aum_usd", { precision: 14, scale: 2 }),
  totalVolume: numeric("total_volume", { precision: 14, scale: 2 }),
  totalPnlUsd: numeric("total_pnl_usd", { precision: 14, scale: 2 }),
  snapshotAt: timestamp("snapshot_at", { withTimezone: true }).defaultNow(),
});

export const strategies = pgTable("strategies", {
  id: text("id").primaryKey(),
  name: text("name").notNull(),
  description: text("description"),
  venues: text("venues").array().notNull(),
  minFundUsd: numeric("min_fund_usd", { precision: 8, scale: 2 }).default("10"),
  riskLevel: text("risk_level").default("medium"),
  configSchema: jsonb("config_schema"),
  performance: jsonb("performance"),
  active: boolean("active").default(true),
});

export const agentMessages = pgTable("agent_messages", {
  id: uuid("id").primaryKey().defaultRandom(),
  deploymentId: uuid("deployment_id").references(() => deployments.id),
  direction: text("direction").notNull(),
  content: text("content").notNull(),
  processed: boolean("processed").default(false),
  ts: timestamp("ts", { withTimezone: true }).defaultNow(),
});

export const siweNonces = pgTable("siwe_nonces", {
  nonce: text("nonce").primaryKey(),
  createdAt: timestamp("created_at", { withTimezone: true }).defaultNow(),
  used: boolean("used").default(false),
});
