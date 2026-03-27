import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments, agentMessages } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { closeAkashDeployment, getAkashBalance } from "@/lib/akash/client";
import { getRedis } from "@/lib/redis";

export const dynamic = "force-dynamic";

export async function POST(req: NextRequest) {
  const auth = await requireAuth();
  if (auth instanceof NextResponse) return auth;

  const { deploymentId } = await req.json();
  const db = getDb();

  const [deployment] = await db.select().from(deployments)
    .where(and(eq(deployments.id, deploymentId), eq(deployments.userAddress, auth.userAddress!)));

  if (!deployment) {
    return NextResponse.json({ error: "Not found" }, { status: 404 });
  }

  if (deployment.status === "terminating" || deployment.status === "terminated") {
    return NextResponse.json({ error: "Already terminating" }, { status: 400 });
  }

  // Check balance before close (for recovery reporting)
  let balanceBefore: number | null = null;
  try {
    const bal = await getAkashBalance();
    balanceBefore = bal?.data?.balance ?? null;
  } catch { /* non-blocking */ }

  // Update status
  await db.update(deployments)
    .set({ status: "terminating" })
    .where(eq(deployments.id, deploymentId));

  // Send shutdown command via Redis (fast channel)
  try {
    const redis = getRedis();
    await redis.publish(`agent:${deploymentId}:commands`, "SYSTEM:SHUTDOWN");
  } catch {
    // Fallback to DB
    await db.insert(agentMessages).values({
      deploymentId,
      direction: "user_to_agent",
      content: "SYSTEM:SHUTDOWN",
    });
  }

  // Close Akash deployment — this returns the escrow deposit
  let akashClosed = false;
  if (deployment.akashDseq) {
    try {
      await closeAkashDeployment(deployment.akashDseq);
      akashClosed = true;
    } catch (error: any) {
      console.error("Akash close failed:", error.message);
      // Non-blocking — deployment may already be closed
    }
  }

  // Check balance after close to report recovered amount
  let recovered: number | null = null;
  if (akashClosed && balanceBefore !== null) {
    try {
      // Small delay for on-chain settlement
      await new Promise((r) => setTimeout(r, 2000));
      const bal = await getAkashBalance();
      const balanceAfter = bal?.data?.balance ?? 0;
      recovered = balanceAfter - balanceBefore;
    } catch { /* non-blocking */ }
  }

  await db.update(deployments)
    .set({ status: "terminated", terminatedAt: new Date() })
    .where(eq(deployments.id, deploymentId));

  return NextResponse.json({
    status: "terminated",
    akashClosed,
    ...(recovered !== null && recovered > 0 ? { recoveredUakt: recovered } : {}),
  });
}
