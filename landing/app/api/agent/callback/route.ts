import { NextRequest, NextResponse } from "next/server";
import { getDb } from "@/lib/db";
import { deployments, trades, agentMessages } from "@/lib/db/schema";
import { eq, and, asc } from "drizzle-orm";

export const dynamic = "force-dynamic";

async function authenticateCallback(req: NextRequest, deploymentId: string) {
  const authHeader = req.headers.get("authorization");
  if (!authHeader?.startsWith("Bearer ")) {
    return null;
  }
  const token = authHeader.slice(7);

  const db = getDb();
  const [deployment] = await db.select().from(deployments)
    .where(eq(deployments.id, deploymentId));

  if (!deployment) return null;

  const config = deployment.config as Record<string, any>;
  if (config?._callbackToken !== token) return null;

  return deployment;
}

// POST — Agent reports trade, heartbeat, or message
export async function POST(req: NextRequest) {
  const body = await req.json();
  const { deploymentId, type, ...payload } = body;

  if (!deploymentId || !type) {
    return NextResponse.json({ error: "deploymentId and type required" }, { status: 400 });
  }

  const deployment = await authenticateCallback(req, deploymentId);
  if (!deployment) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }

  const db = getDb();

  switch (type) {
    case "trade": {
      const { venue, action, amountUsd, pnlUsd } = payload;
      await db.insert(trades).values({
        deploymentId,
        venue: venue ?? "unknown",
        action: action ?? "unknown",
        amountUsd: amountUsd?.toString() ?? "0",
        pnlUsd: pnlUsd?.toString() ?? "0",
      });
      return NextResponse.json({ ok: true });
    }

    case "heartbeat": {
      const { phase } = payload; // "bootstrap" | "ready" | "trading"
      // Store last heartbeat in deployment config
      const config = deployment.config as Record<string, any>;
      await db.update(deployments)
        .set({
          config: {
            ...config,
            _lastHeartbeat: new Date().toISOString(),
            _phase: phase ?? "active",
          },
        })
        .where(eq(deployments.id, deploymentId));
      return NextResponse.json({ ok: true });
    }

    case "message": {
      const { content } = payload;
      if (!content) {
        return NextResponse.json({ error: "content required" }, { status: 400 });
      }
      await db.insert(agentMessages).values({
        deploymentId,
        direction: "agent_to_user",
        content,
      });
      return NextResponse.json({ ok: true });
    }

    default:
      return NextResponse.json({ error: `Unknown type: ${type}` }, { status: 400 });
  }
}

// GET — Agent polls for pending user commands
export async function GET(req: NextRequest) {
  const deploymentId = req.nextUrl.searchParams.get("deploymentId");
  if (!deploymentId) {
    return NextResponse.json({ error: "deploymentId required" }, { status: 400 });
  }

  const deployment = await authenticateCallback(req, deploymentId);
  if (!deployment) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }

  const db = getDb();

  // Get unprocessed user→agent messages
  const pending = await db.select().from(agentMessages)
    .where(
      and(
        eq(agentMessages.deploymentId, deploymentId),
        eq(agentMessages.direction, "user_to_agent"),
        eq(agentMessages.processed, false),
      )
    )
    .orderBy(asc(agentMessages.ts))
    .limit(10);

  // Mark as processed
  if (pending.length > 0) {
    const ids = pending.map((m) => m.id);
    for (const id of ids) {
      await db.update(agentMessages)
        .set({ processed: true })
        .where(eq(agentMessages.id, id));
    }
  }

  return NextResponse.json({ commands: pending });
}
