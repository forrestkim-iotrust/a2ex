import { privateKeyToAccount } from "viem/accounts";
import { SiweMessage } from "siwe";

const BASE = process.env.BASE_URL || "https://landing-two-silk-25.vercel.app";
const PRIVATE_KEY = (process.env.PRIVATE_KEY || "0x492a11a942c5b403a5e53203501d78f6f13d7c5df9588ff139406b6d8b03c56c") as `0x${string}`;
const SIWE_DOMAIN = process.env.SIWE_DOMAIN || new URL(BASE).hostname;

const account = privateKeyToAccount(PRIVATE_KEY);

function parseCookie(res: Response): string {
  const setCookies = res.headers.getSetCookie?.() ?? [];
  for (const sc of setCookies) {
    if (sc.startsWith("a2ex-session=")) return sc.split(";")[0];
  }
  const raw = res.headers.get("set-cookie") ?? "";
  const m = raw.match(/a2ex-session=[^;]+/);
  return m ? m[0] : "";
}

async function main() {
  const nr = await fetch(`${BASE}/api/auth/siwe`);
  if (!nr.ok) { console.error("Nonce failed:", nr.status); process.exit(1); }

  let cookie = parseCookie(nr);
  const { nonce } = await nr.json() as { nonce: string };

  const msg = new SiweMessage({
    domain: SIWE_DOMAIN,
    address: account.address,
    statement: "Sign in to A2EX",
    uri: BASE,
    version: "1",
    chainId: 1,
    nonce,
    issuedAt: new Date().toISOString(),
  });

  const message = msg.prepareMessage();
  const signature = await account.signMessage({ message });

  const vr = await fetch(`${BASE}/api/auth/siwe`, {
    method: "POST",
    headers: { "Content-Type": "application/json", cookie },
    body: JSON.stringify({ message, signature }),
  });

  if (!vr.ok) {
    console.error("Verify failed:", vr.status, await vr.text());
    process.exit(1);
  }

  const authCookie = parseCookie(vr) || cookie;
  const result = await vr.json() as Record<string, unknown>;

  if (result.ok) {
    // Output just the cookie value for piping
    console.log(authCookie);
  } else {
    console.error("Auth failed:", JSON.stringify(result));
    process.exit(1);
  }
}

main();
