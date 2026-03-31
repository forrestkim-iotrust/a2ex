import { NextRequest, NextResponse } from "next/server";
import { getDb } from "@/lib/db";
import { deployments, trades, agentMessages } from "@/lib/db/schema";
import { eq, and, asc, inArray } from "drizzle-orm";
import { getRedis } from "@/lib/redis";
import { Ratelimit } from "@upstash/ratelimit";
import { log } from "@/lib/log";

export const dynamic = "force-dynamic";

// Rate limit: 10 requests per second per deployment
let ratelimit: Ratelimit | null = null;
function getRatelimit(): Ratelimit {
  if (!ratelimit) {
    ratelimit = new Ratelimit({
      redis: getRedis(),
      limiter: Ratelimit.slidingWindow(10, "1 s"),
      prefix: "cb",
    });
  }
  return ratelimit;
}

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

// Publish event to Redis for realtime subscribers
async function publishEvent(deploymentId: string, type: string, data: Record<string, any>) {
  try {
    const redis = getRedis();
    await redis.publish(`deploy:${deploymentId}`, JSON.stringify({ type, ...data, ts: Date.now() }));
  } catch { /* non-blocking — DB write already succeeded */ }
}

// POST — Agent reports trade, heartbeat, message, balance_update, or backup
export async function POST(req: NextRequest) {
  const body = await req.json();
  const { deploymentId, type, ...payload } = body;

  if (!deploymentId || !type) {
    return NextResponse.json({ error: "deploymentId and type required" }, { status: 400 });
  }

  // Rate limit check
  try {
    const { success } = await getRatelimit().limit(deploymentId);
    if (!success) {
      return NextResponse.json({ error: "Rate limited" }, { status: 429 });
    }
  } catch { /* if rate limit fails, allow through */ }

  const deployment = await authenticateCallback(req, deploymentId);
  if (!deployment) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }

  const db = getDb();

  switch (type) {
    case "trade": {
      const { venue, action, amountUsd, pnlUsd } = payload;
      log("callback.trade", { deploymentId, venue, action, amountUsd });
      const [trade] = await db.insert(trades).values({
        deploymentId,
        venue: venue ?? "unknown",
        action: action ?? "unknown",
        amountUsd: amountUsd?.toString() ?? "0",
        pnlUsd: pnlUsd?.toString() ?? "0",
      }).returning();
      await publishEvent(deploymentId, "trade", trade);
      return NextResponse.json({ ok: true });
    }

    case "heartbeat": {
      const { phase } = payload;
      log("callback.heartbeat", { deploymentId, phase });
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
      await publishEvent(deploymentId, "heartbeat", { phase: phase ?? "active" });
      return NextResponse.json({ ok: true });
    }

    case "message": {
      const { content } = payload;
      if (!content) {
        return NextResponse.json({ error: "content required" }, { status: 400 });
      }
      const [msg] = await db.insert(agentMessages).values({
        deploymentId,
        direction: "agent_to_user",
        content,
      }).returning();
      await publishEvent(deploymentId, "message", msg);
      return NextResponse.json({ ok: true });
    }

    case "balance_update": {
      const { usdcBalance } = payload;
      log("callback.balance_update", { deploymentId, usdcBalance });
      const config = deployment.config as Record<string, any>;
      await db.update(deployments)
        .set({
          config: {
            ...config,
            _usdcBalance: usdcBalance?.toString() ?? "0",
            _lastBalanceUpdate: new Date().toISOString(),
          },
        })
        .where(eq(deployments.id, deploymentId));
      await publishEvent(deploymentId, "balance_update", { usdcBalance });
      return NextResponse.json({ ok: true });
    }

    case "backup": {
      const { encryptedData } = payload;
      if (!encryptedData) {
        return NextResponse.json({ error: "encryptedData required" }, { status: 400 });
      }
      log("callback.backup", { deploymentId, size: encryptedData.length });
      const config = deployment.config as Record<string, any>;
      await db.update(deployments)
        .set({
          encryptedBackup: encryptedData,
          config: {
            ...config,
            _lastBackupAt: new Date().toISOString(),
          },
        })
        .where(eq(deployments.id, deploymentId));
      await publishEvent(deploymentId, "backup", { success: true });
      return NextResponse.json({ ok: true });
    }

    default:
      return NextResponse.json({ error: `Unknown type: ${type}` }, { status: 400 });
  }
}

// GET — Agent polls for pending user commands or requests secrets
export async function GET(req: NextRequest) {
  const deploymentId = req.nextUrl.searchParams.get("deploymentId");
  if (!deploymentId) {
    return NextResponse.json({ error: "deploymentId required" }, { status: 400 });
  }

  const deployment = await authenticateCallback(req, deploymentId);
  if (!deployment) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }

  const reqType = req.nextUrl.searchParams.get("type");

  if (reqType === "secrets") {
    log("callback.secrets_requested", { deploymentId });
    const config = deployment.config as Record<string, any>;
    return NextResponse.json({
      openrouterApiKey: config?._openrouterApiKey ?? "",
      waiaasPassword: config?._waiaasPassword ?? "",
      gatewayToken: config?._gatewayToken ?? "",
      backupKey: config?._backupKey ?? "",
    });
  }

  const db = getDb();

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

  if (pending.length > 0) {
    const ids = pending.map((m) => m.id);
    await db.update(agentMessages)
      .set({ processed: true })
      .where(inArray(agentMessages.id, ids));
  }

  return NextResponse.json({ commands: pending });
}
