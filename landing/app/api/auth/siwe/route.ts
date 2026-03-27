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
  const { data: verified } = await siweMessage.verify({ signature, nonce: session.nonce });

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

export async function DELETE() {
  const session = await getSession();
  session.destroy();
  return NextResponse.json({ ok: true });
}
