import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { agentMessages, deployments } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { getRedis } from "@/lib/redis";

export const dynamic = "force-dynamic";

export async function POST(req: NextRequest) {
  const auth = await requireAuth();
  if (auth instanceof NextResponse) return auth;

  const { deploymentId, content } = await req.json();

  if (!content || content.length > 500) {
    return NextResponse.json({ error: "Message must be 1-500 characters" }, { status: 400 });
  }

  const db = getDb();

  // Verify deployment belongs to user
  const [deployment] = await db.select().from(deployments)
    .where(and(eq(deployments.id, deploymentId), eq(deployments.userAddress, auth.userAddress!)));

  if (!deployment) {
    return NextResponse.json({ error: "Not found" }, { status: 404 });
  }

  // Store in DB for history
  const [message] = await db.insert(agentMessages).values({
    deploymentId,
    direction: "user_to_agent",
    content,
  }).returning();

  // Publish via Redis for real-time delivery
  try {
    const redis = getRedis();
    await redis.publish(`agent:${deploymentId}:commands`, content);
  } catch {
    // Redis failure is non-blocking — agent polls DB as fallback
  }

  return NextResponse.json(message);
}
