import { NextResponse } from "next/server";
import { getDb } from "@/lib/db";
import { statsSnapshots } from "@/lib/db/schema";
import { desc } from "drizzle-orm";

export const dynamic = "force-dynamic";

export async function GET() {
  const db = getDb();

  const [latest] = await db.select().from(statsSnapshots)
    .orderBy(desc(statsSnapshots.snapshotAt))
    .limit(1);

  if (!latest) {
    return NextResponse.json({
      totalAgents: 0,
      totalAumUsd: "0",
      totalVolume: "0",
      totalPnlUsd: "0",
    });
  }

  return NextResponse.json(latest);
}
