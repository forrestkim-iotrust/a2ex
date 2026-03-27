import { NextRequest, NextResponse } from "next/server";
import { getDb } from "@/lib/db";
import { deployments, trades, statsSnapshots } from "@/lib/db/schema";
import { eq, sql } from "drizzle-orm";

export async function GET(req: NextRequest) {
  // Verify cron secret
  const authHeader = req.headers.get("authorization");
  if (authHeader !== `Bearer ${process.env.CRON_SECRET}`) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }

  const db = getDb();

  const activeAgents = await db.select({ count: sql<number>`count(*)` })
    .from(deployments)
    .where(eq(deployments.status, "active"));

  const aumResult = await db.select({
    totalAum: sql<string>`coalesce(sum((${deployments.config}->>'fundAmountUsd')::numeric), 0)`,
  }).from(deployments)
    .where(eq(deployments.status, "active"));

  const tradeStats = await db.select({
    totalVolume: sql<string>`coalesce(sum(abs(${trades.amountUsd})), 0)`,
    totalPnl: sql<string>`coalesce(sum(${trades.pnlUsd}), 0)`,
  }).from(trades);

  await db.insert(statsSnapshots).values({
    totalAgents: activeAgents[0]?.count ?? 0,
    totalAumUsd: aumResult[0]?.totalAum ?? "0",
    totalVolume: tradeStats[0]?.totalVolume ?? "0",
    totalPnlUsd: tradeStats[0]?.totalPnl ?? "0",
  });

  return NextResponse.json({ ok: true });
}
