import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getSession } from "@/lib/auth/session";
import { getDb } from "@/lib/db";
import { deployments } from "@/lib/db/schema";
import { eq } from "drizzle-orm";
import { createAkashDeployment } from "@/lib/akash/client";
import { buildSDL } from "@/lib/akash/sdl";
import { log } from "@/lib/log";

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

  log("deploy.created", { deploymentId: deployment.id, userAddress: auth.userAddress, strategyId });

  try {
    // Generate per-deployment secrets
    const gatewayToken = crypto.randomUUID();
    const callbackToken = crypto.randomUUID();
    const waiaasPassword = crypto.randomUUID();

    // Get backup key from session (derived from personal_sign)
    const session = await getSession();
    const backupKey = session.backupKey;

    // Build SDL (secrets delivered via callback, not in SDL)
    const sdl = buildSDL({
      strategyId,
      fundAmountUsd: config.fundAmountUsd ?? 50,
      riskLevel: config.riskLevel ?? "medium",
      openclawGatewayToken: gatewayToken,
      callbackUrl: `${process.env.NEXT_PUBLIC_BASE_URL || "https://a2ex.xyz"}/api/agent/callback`,
      deploymentId: deployment.id,
      callbackToken,
    });

    await db.update(deployments)
      .set({ status: "sdl_generated" })
      .where(eq(deployments.id, deployment.id));

    // Submit to Akash — returns fast (~5s), does NOT wait for bids
    const result = await createAkashDeployment(sdl);
    const dseq = result?.data?.dseq?.toString();
    const manifest = result?.data?.manifest;

    if (!dseq) throw new Error("No dseq returned from Akash");

    // Store dseq + manifest for later bid acceptance
    await db.update(deployments)
      .set({
        status: "awaiting_bids",
        akashDseq: dseq,
        config: { ...config, _manifest: manifest, _gatewayToken: gatewayToken, _callbackToken: callbackToken, _openrouterApiKey: process.env.OPENROUTER_API_KEY, _waiaasPassword: waiaasPassword, ...(backupKey ? { _backupKey: backupKey } : {}) },
      })
      .where(eq(deployments.id, deployment.id));

    log("deploy.submitted", { deploymentId: deployment.id, dseq });

    // Return immediately — dashboard will poll /api/deploy/progress to advance
    return NextResponse.json({
      id: deployment.id,
      akashDseq: dseq,
      status: "awaiting_bids",
    });
  } catch (error: any) {
    log("deploy.failed", { deploymentId: deployment.id, error: error.message });

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
