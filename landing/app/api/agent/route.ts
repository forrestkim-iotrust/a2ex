import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments, trades, agentMessages } from "@/lib/db/schema";
import { eq, desc, and } from "drizzle-orm";

export async function GET(req: NextRequest) {
  const auth = await requireAuth();
  if (auth instanceof NextResponse) return auth;

  const deploymentId = req.nextUrl.searchParams.get("deploymentId");
  if (!deploymentId) {
    return NextResponse.json({ error: "deploymentId required" }, { status: 400 });
  }

  const db = getDb();

  const [deployment] = await db.select().from(deployments)
    .where(and(eq(deployments.id, deploymentId), eq(deployments.userAddress, auth.userAddress!)));

  if (!deployment) {
    return NextResponse.json({ error: "Not found" }, { status: 404 });
  }

  const recentTrades = await db.select().from(trades)
    .where(eq(trades.deploymentId, deploymentId))
    .orderBy(desc(trades.ts))
    .limit(50);

  const messages = await db.select().from(agentMessages)
    .where(eq(agentMessages.deploymentId, deploymentId))
    .orderBy(desc(agentMessages.ts))
    .limit(40);

  return NextResponse.json({ deployment, trades: recentTrades, messages });
}
