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

  // Strip internal secrets from config before returning
  const secretKeys = ["_callbackToken", "_gatewayToken", "_manifest", "_openrouterApiKey", "_waiaasPassword", "_backupKey"];
  let safeConfig = deployment.config;
  if (safeConfig && typeof safeConfig === "object") {
    const filtered = { ...(safeConfig as Record<string, any>) };
    for (const key of secretKeys) {
      delete filtered[key];
    }
    safeConfig = filtered;
  }

  return NextResponse.json({ deployment: { ...deployment, config: safeConfig }, trades: recentTrades, messages });
}
