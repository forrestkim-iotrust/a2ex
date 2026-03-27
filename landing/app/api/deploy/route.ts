import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments } from "@/lib/db/schema";
import { eq } from "drizzle-orm";
import { createAkashDeployment } from "@/lib/akash/client";
import { buildSDL } from "@/lib/akash/sdl";

export const dynamic = "force-dynamic";

export async function POST(req: NextRequest) {
  const auth = await requireAuth();
  if (auth instanceof NextResponse) return auth;

  const { strategyId, config } = await req.json();
  const db = getDb();

  // Create deployment record
  const [deployment] = await db.insert(deployments).values({
    userAddress: auth.userAddress!,
    strategyId,
    config,
    status: "pending",
  }).returning();

  try {
    // Build SDL
    const sdl = buildSDL({
      strategyId,
      fundAmountUsd: config.fundAmountUsd ?? 50,
      riskLevel: config.riskLevel ?? "medium",
    });

    await db.update(deployments)
      .set({ status: "sdl_generated" })
      .where(eq(deployments.id, deployment.id));

    // Deploy to Akash
    const result = await createAkashDeployment(sdl);
    const dseq = result?.data?.deploymentDseq || result?.data?.dseq;

    await db.update(deployments)
      .set({ status: "bid_received", akashDseq: dseq?.toString() })
      .where(eq(deployments.id, deployment.id));

    return NextResponse.json({
      deploymentId: deployment.id,
      akashDseq: dseq,
      status: "bid_received",
    });
  } catch (error: any) {
    await db.update(deployments)
      .set({ status: "failed" })
      .where(eq(deployments.id, deployment.id));

    return NextResponse.json(
      { error: error.message, deploymentId: deployment.id, status: "failed" },
      { status: 500 }
    );
  }
}

export async function GET(req: NextRequest) {
  const auth = await requireAuth();
  if (auth instanceof NextResponse) return auth;

  const db = getDb();
  const userDeployments = await db.select().from(deployments)
    .where(eq(deployments.userAddress, auth.userAddress!))
    .orderBy(deployments.createdAt);

  return NextResponse.json(userDeployments);
}
