import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { getAkashBids, createAkashLease, closeAkashDeployment } from "@/lib/akash/client";

export const dynamic = "force-dynamic";

function cheapestOpenBid(bids: any[]) {
  const open = bids.filter((b: any) => b.bid?.state === "open");
  if (open.length === 0) return null;
  return open.sort((a: any, b: any) =>
    parseFloat(a.bid.price.amount) - parseFloat(b.bid.price.amount)
  )[0];
}

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

  // Already terminal
  if (["active", "terminated", "failed"].includes(deployment.status)) {
    return NextResponse.json({ status: deployment.status, akashDseq: deployment.akashDseq });
  }

  // Deploying states — advance lifecycle
  if ((deployment.status === "awaiting_bids" || deployment.status === "selecting_provider") && deployment.akashDseq) {
    try {
      const result = await getAkashBids(deployment.akashDseq);
      const bids = result?.data ?? [];
      const totalBids = bids.length;

      // No bids at all yet
      if (totalBids === 0) {
        return NextResponse.json({
          status: "awaiting_bids",
          akashDseq: deployment.akashDseq,
          bidCount: 0,
        });
      }

      // Find cheapest open bid
      const best = cheapestOpenBid(bids);

      if (!best) {
        // Bids exist but none are open — all expired
        await db.update(deployments)
          .set({ status: "failed" })
          .where(eq(deployments.id, deploymentId));

        // Close the Akash deployment to recover deposit
        try { await closeAkashDeployment(deployment.akashDseq); } catch { /* best-effort */ }

        return NextResponse.json({
          status: "failed",
          error: "All provider bids expired. Deployment closed.",
          bidCount: totalBids,
        });
      }

      // Update to selecting_provider
      if (deployment.status === "awaiting_bids") {
        await db.update(deployments)
          .set({ status: "selecting_provider" })
          .where(eq(deployments.id, deploymentId));
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
        bidCount: totalBids,
      });
    } catch (error: any) {
      // Lease creation failed — mark as failed and close Akash deployment
      await db.update(deployments)
        .set({ status: "failed" })
        .where(eq(deployments.id, deploymentId));

      try { await closeAkashDeployment(deployment.akashDseq!); } catch { /* best-effort */ }

      return NextResponse.json({
        status: "failed",
        error: error.message,
      });
    }
  }

  // Other intermediate states
  return NextResponse.json({ status: deployment.status, akashDseq: deployment.akashDseq });
}
