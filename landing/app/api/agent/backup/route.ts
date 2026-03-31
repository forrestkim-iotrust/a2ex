import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";

export const dynamic = "force-dynamic";

// GET — User fetches encrypted backup for recovery
export async function GET(req: NextRequest) {
  const auth = await requireAuth();
  if (auth instanceof NextResponse) return auth;

  const deploymentId = req.nextUrl.searchParams.get("deploymentId");
  if (!deploymentId) {
    return NextResponse.json({ error: "deploymentId required" }, { status: 400 });
  }

  const db = getDb();
  const [deployment] = await db.select({
    id: deployments.id,
    encryptedBackup: deployments.encryptedBackup,
    status: deployments.status,
  }).from(deployments)
    .where(and(eq(deployments.id, deploymentId), eq(deployments.userAddress, auth.userAddress!)));

  if (!deployment) {
    return NextResponse.json({ error: "Not found" }, { status: 404 });
  }

  if (!deployment.encryptedBackup) {
    return NextResponse.json({ error: "No backup available" }, { status: 404 });
  }

  return NextResponse.json({
    deploymentId: deployment.id,
    encryptedBackup: deployment.encryptedBackup,
    status: deployment.status,
  });
}
