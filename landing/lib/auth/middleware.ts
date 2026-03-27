import { getSession } from "./session";
import { NextResponse } from "next/server";

export async function requireAuth() {
  const session = await getSession();
  if (!session.userAddress) {
    return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
  }
  return session;
}
