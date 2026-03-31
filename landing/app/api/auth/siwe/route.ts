import { NextRequest, NextResponse } from "next/server";
import { SiweMessage, generateNonce } from "siwe";
import { getSession } from "@/lib/auth/session";
import { getDb } from "@/lib/db";
import { siweNonces } from "@/lib/db/schema";
import { eq } from "drizzle-orm";

export async function GET() {
  const nonce = generateNonce();
  const db = getDb();
  await db.insert(siweNonces).values({ nonce });
  const session = await getSession();
  session.nonce = nonce;
  await session.save();
  return NextResponse.json({ nonce });
}

export async function POST(req: NextRequest) {
  const { message, signature } = await req.json();
  const session = await getSession();

  const siweMessage = new SiweMessage(message);
  const { data: verified } = await siweMessage.verify({
    signature,
    nonce: session.nonce,
    domain: new URL(process.env.NEXT_PUBLIC_BASE_URL || 'https://a2ex.xyz').hostname,
  });

  // Check nonce hasn't been used
  const db = getDb();
  const [nonceRecord] = await db.select().from(siweNonces).where(eq(siweNonces.nonce, verified.nonce));
  if (!nonceRecord || nonceRecord.used) {
    return NextResponse.json({ error: "Invalid or used nonce" }, { status: 401 });
  }

  // Mark nonce as used
  await db.update(siweNonces).set({ used: true }).where(eq(siweNonces.nonce, verified.nonce));

  // Create session
  session.userAddress = verified.address;
  session.chainId = verified.chainId;
  session.sessionCreatedAt = Date.now();
  session.nonce = undefined;
  await session.save();

  return NextResponse.json({ ok: true, address: verified.address });
}

// PUT — Store backup key derived from personal_sign
export async function PUT(req: NextRequest) {
  const session = await getSession();
  if (!session.userAddress) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }

  const { backupKey } = await req.json();
  if (!backupKey || typeof backupKey !== "string" || backupKey.length < 32) {
    return NextResponse.json({ error: "Invalid backup key" }, { status: 400 });
  }

  session.backupKey = backupKey;
  await session.save();

  return NextResponse.json({ ok: true });
}

export async function DELETE() {
  const session = await getSession();
  session.destroy();
  return NextResponse.json({ ok: true });
}
