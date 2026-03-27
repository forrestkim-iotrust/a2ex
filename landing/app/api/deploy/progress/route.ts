import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { getAkashBids, createAkashLease } from "@/lib/akash/client";

export const dynamic = "force-dynamic";

function cheapestBid(bids: any[]) {
  return bids
    .filter((b: any) => b.bid?.state === "open")
    .sort((a: any, b: any) =>
      parseFloat(a.bid.price.amount) - parseFloat(b.bid.price.amount)
    )[0];
}

// GET /api/deploy/progress?deploymentId=X
// Called by dashboard to advance deployment through lifecycle stages
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

  // Already terminal — just return status
  if (["active", "terminated", "failed"].includes(deployment.status)) {
    return NextResponse.json({
      status: deployment.status,
      akashDseq: deployment.akashDseq,
    });
  }

  // Status: awaiting_bids — check for bids and accept cheapest
  if (deployment.status === "awaiting_bids" && deployment.akashDseq) {
    try {
      const result = await getAkashBids(deployment.akashDseq);
      const bids = result?.data ?? [];

      if (bids.length === 0) {
        return NextResponse.json({
          status: "awaiting_bids",
          akashDseq: deployment.akashDseq,
          bidCount: 0,
        });
      }

      // Bids received — update status
      await db.update(deployments)
        .set({ status: "selecting_provider" })
        .where(eq(deployments.id, deploymentId));

      // Select cheapest provider
      const best = cheapestBid(bids);
      if (!best) {
        return NextResponse.json({
          status: "selecting_provider",
          akashDseq: deployment.akashDseq,
          bidCount: bids.length,
        });
      }

      const { provider, gseq, oseq } = best.bid.id;
      const config = deployment.config as Record<string, any>;
      const manifest = config?._manifest;

      // Create lease
      await createAkashLease(deployment.akashDseq, provider, gseq, oseq, manifest);

      await db.update(deployments)
        .set({ status: "active" })
        .where(eq(deployments.id, deploymentId));

      return NextResponse.json({
        status: "active",
        akashDseq: deployment.akashDseq,
        provider,
        bidCount: bids.length,
      });
    } catch (error: any) {
      await db.update(deployments)
        .set({ status: "failed" })
        .where(eq(deployments.id, deploymentId));

      return NextResponse.json({
        status: "failed",
        error: error.message,
      });
    }
  }

  // Other intermediate states — just return current status
  return NextResponse.json({
    status: deployment.status,
    akashDseq: deployment.akashDseq,
  });
}
