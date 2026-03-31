import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { getAkashBids, createAkashLease, closeAkashDeployment, bestOpenBid } from "@/lib/akash/client";

export const dynamic = "force-dynamic";
export const maxDuration = 300;

const BID_POLL_INTERVAL = 3000;
const BID_TIMEOUT = 120000;

function sseEvent(id: number, event: string, data: Record<string, unknown>): string {
  return `id: ${id}\nevent: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
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

  // Already terminal or active — return JSON, no need for SSE
  if (["active", "terminated", "failed"].includes(deployment.status)) {
    return NextResponse.json({ status: deployment.status, akashDseq: deployment.akashDseq });
  }

  // Not in a deploying state — return current status
  if (deployment.status !== "awaiting_bids" && deployment.status !== "selecting_provider") {
    return NextResponse.json({ status: deployment.status, akashDseq: deployment.akashDseq });
  }

  // Acquire lock: atomically transition awaiting_bids → selecting_provider
  if (deployment.status === "awaiting_bids") {
    const [locked] = await db.update(deployments)
      .set({ status: "selecting_provider" })
      .where(and(eq(deployments.id, deploymentId), eq(deployments.status, "awaiting_bids")))
      .returning();

    if (!locked) {
      // Another stream owns the lifecycle
      return NextResponse.json({ status: "selecting_provider", akashDseq: deployment.akashDseq });
    }
  } else {
    // Already selecting_provider — another stream may be active
    return NextResponse.json({ status: "selecting_provider", akashDseq: deployment.akashDseq });
  }

  // --- Phase 1 SSE: single owner drives bid→lease→active ---
  const encoder = new TextEncoder();
  let eventId = 0;
  let closed = false;

  const stream = new ReadableStream({
    start(controller) {
      const send = (event: string, data: Record<string, unknown>): boolean => {
        if (closed) return false;
        try {
          controller.enqueue(encoder.encode(sseEvent(++eventId, event, data)));
          return true;
        } catch {
          closed = true;
          return false;
        }
      };

      // Set retry interval for auto-reconnect (15s)
      try { controller.enqueue(encoder.encode("retry: 15000\n\n")); } catch { closed = true; }

      // Send initial status immediately (before any async work)
      send("status", { status: "selecting_provider", akashDseq: deployment.akashDseq });

      // Fire-and-forget: async lifecycle runs without blocking start()
      // This ensures Next.js flushes the response immediately
      (async () => {
        try {
          // Wait for bids
          let best: any = null;
          let totalBids = 0;
          const startTime = Date.now();

          while (!best && !closed && Date.now() - startTime < BID_TIMEOUT) {
            const result = await getAkashBids(deployment.akashDseq!);
            const bids = result?.data ?? [];
            totalBids = bids.length;

            if (!send("bids", { bidCount: totalBids, elapsed: Date.now() - startTime })) break;

            if (totalBids > 0) {
              best = await bestOpenBid(bids);
              if (best) break;

              const hasOpen = bids.some((b: any) => b.bid?.state === "open");
              if (!hasOpen) {
                await db.update(deployments).set({ status: "failed" }).where(eq(deployments.id, deploymentId));
                try { await closeAkashDeployment(deployment.akashDseq!); } catch {}
                send("failed", { error: "All provider bids expired.", bidCount: totalBids });
                controller.close();
                return;
              }
            }

            await new Promise((r) => setTimeout(r, BID_POLL_INTERVAL));
          }

          if (closed) { try { controller.close(); } catch {} return; }

          if (!best) {
            await db.update(deployments).set({ status: "failed" }).where(eq(deployments.id, deploymentId));
            try { await closeAkashDeployment(deployment.akashDseq!); } catch {}
            send("failed", { error: "Bid timeout — no suitable provider found.", bidCount: totalBids });
            controller.close();
            return;
          }

          // Create lease
          const { provider, gseq, oseq } = best.bid.id;
          send("status", { status: "creating_lease", provider, bidCount: totalBids });

          const config = deployment.config as Record<string, any>;
          const manifest = config?._manifest;
          const leaseResult = await createAkashLease(deployment.akashDseq!, provider, gseq, oseq, manifest);

          // Extract forwarded ports
          const leaseStatus = leaseResult?.data?.leases?.[0]?.status;
          const ports = leaseStatus?.forwarded_ports;
          let gatewayUrl: string | undefined;
          if (ports) {
            const svcPorts = Object.values(ports).flat() as any[];
            const gwPort = svcPorts.find((p: any) => p.port === 18789);
            if (gwPort) gatewayUrl = `http://${gwPort.host}:${gwPort.externalPort}`;
          }

          // Mark active
          const updatedConfig = { ...config, _provider: provider, _gatewayUrl: gatewayUrl, _ports: ports };
          await db.update(deployments)
            .set({ status: "active", config: updatedConfig })
            .where(eq(deployments.id, deploymentId));

          send("active", {
            status: "active",
            akashDseq: deployment.akashDseq,
            provider,
            gatewayUrl,
            bidCount: totalBids,
          });

          controller.close();
        } catch (error: any) {
          // Check if another path already succeeded
          const [current] = await db.select({ status: deployments.status }).from(deployments)
            .where(eq(deployments.id, deploymentId));

          if (current?.status !== "active" && current?.status !== "terminated") {
            await db.update(deployments).set({ status: "failed" }).where(eq(deployments.id, deploymentId));
            try { await closeAkashDeployment(deployment.akashDseq!); } catch {}
          }

          send("failed", { error: error.message });
          try { controller.close(); } catch {}
        }
      })();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}
