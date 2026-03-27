import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments } from "@/lib/db/schema";
import { eq } from "drizzle-orm";
import { createAkashDeployment, getAkashBids, createAkashLease } from "@/lib/akash/client";
import { buildSDL } from "@/lib/akash/sdl";

export const dynamic = "force-dynamic";
export const maxDuration = 60; // Allow up to 60s for bid polling

async function pollBids(dseq: string, maxAttempts = 10, intervalMs = 3000) {
  for (let i = 0; i < maxAttempts; i++) {
    const result = await getAkashBids(dseq);
    const bids = result?.data ?? [];
    if (bids.length > 0) return bids;
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  return [];
}

function cheapestBid(bids: any[]) {
  return bids
    .filter((b: any) => b.bid?.state === "open")
    .sort((a: any, b: any) =>
      parseFloat(a.bid.price.amount) - parseFloat(b.bid.price.amount)
    )[0];
}

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
    const dseq = result?.data?.dseq?.toString();
    const manifest = result?.data?.manifest;

    if (!dseq) throw new Error("No dseq returned from Akash");

    await db.update(deployments)
      .set({ status: "bid_received", akashDseq: dseq })
      .where(eq(deployments.id, deployment.id));

    // Poll for bids
    const bids = await pollBids(dseq);
    if (bids.length === 0) {
      await db.update(deployments)
        .set({ status: "failed" })
        .where(eq(deployments.id, deployment.id));
      return NextResponse.json({
        id: deployment.id,
        error: "No provider bids received within timeout",
        status: "failed",
      });
    }

    // Select cheapest provider
    const best = cheapestBid(bids);
    if (!best) throw new Error("No open bids available");

    const { provider, gseq, oseq } = best.bid.id;

    // Create lease
    await createAkashLease(dseq, provider, gseq, oseq, manifest);

    await db.update(deployments)
      .set({ status: "active" })
      .where(eq(deployments.id, deployment.id));

    return NextResponse.json({
      id: deployment.id,
      akashDseq: dseq,
      provider,
      status: "active",
    });
  } catch (error: any) {
    await db.update(deployments)
      .set({ status: "failed" })
      .where(eq(deployments.id, deployment.id));

    return NextResponse.json({
      id: deployment.id,
      error: error.message,
      status: "failed",
    });
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
