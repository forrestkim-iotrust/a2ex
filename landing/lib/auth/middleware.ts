import { getSession } from "./session";
import { NextResponse } from "next/server";
import type { SessionData } from "./session";

export async function requireAuth(): Promise<NextResponse | SessionData> {
  // Test mode: bypass SIWE when TEST_WALLET_ADDRESS is set (server-side only)
  if (process.env.TEST_WALLET_ADDRESS) {
    return {
      userAddress: process.env.TEST_WALLET_ADDRESS,
      chainId: 42161, // Arbitrum
    };
  }

  const session = await getSession();
  if (!session.userAddress) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }
  return session;
}
