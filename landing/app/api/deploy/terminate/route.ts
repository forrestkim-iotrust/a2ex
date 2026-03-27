import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments, agentMessages } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { closeAkashDeployment } from "@/lib/akash/client";
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

  // Close Akash deployment if dseq exists
  if (deployment.akashDseq) {
    try {
      await closeAkashDeployment(deployment.akashDseq);
    } catch (error: any) {
      console.error("Akash close failed:", error.message);
      // Non-blocking — deployment may already be closed
    }
  }

  await db.update(deployments)
    .set({ status: "terminated", terminatedAt: new Date() })
    .where(eq(deployments.id, deploymentId));

  return NextResponse.json({ status: "terminated" });
}
