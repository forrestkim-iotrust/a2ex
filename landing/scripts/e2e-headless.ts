/**
 * Headless E2E test: SIWE auth → Deploy agent → Monitor SSE → Verify dashboard
 *
 * Usage: PRIVATE_KEY=0x... BASE_URL=https://... npx tsx scripts/e2e-headless.ts
 */
import { privateKeyToAccount } from "viem/accounts";
import { SiweMessage } from "siwe";
import { createHash } from "node:crypto";

const PRIVATE_KEY = (process.env.PRIVATE_KEY || "0x492a11a942c5b403a5e53203501d78f6f13d7c5df9588ff139406b6d8b03c56c") as `0x${string}`;
const BASE_URL = process.env.BASE_URL || "https://landing-two-silk-25.vercel.app";
// SIWE domain must match the server's NEXT_PUBLIC_BASE_URL, not the request URL
const SIWE_DOMAIN = process.env.SIWE_DOMAIN || new URL(BASE_URL).hostname;

const account = privateKeyToAccount(PRIVATE_KEY);
let sessionCookie = "";

function extractCookie(res: Response): string {
  const raw = res.headers.get("set-cookie") ?? "";
  const match = raw.match(/a2ex-session=[^;]+/);
  return match ? match[0] : sessionCookie;
}

function log(step: string, data?: unknown) {
  console.log(`[E2E] ${step}`, data ? JSON.stringify(data) : "");
}

async function step1_siweAuth(): Promise<boolean> {
  log("Step 1: SIWE Authentication");
  log(`  Wallet: ${account.address}`);

  // Get nonce
  const nonceRes = await fetch(`${BASE_URL}/api/auth/siwe`);
  if (!nonceRes.ok) throw new Error(`Nonce failed: ${nonceRes.status}`);
  sessionCookie = extractCookie(nonceRes);
  const { nonce } = await nonceRes.json() as { nonce: string };
  log(`  Nonce: ${nonce}`);

  // Construct SIWE message
  const domain = SIWE_DOMAIN;
  const siweMsg = new SiweMessage({
    domain,
    address: account.address,
    statement: "Sign in to A2EX",
    uri: BASE_URL,
    version: "1",
    chainId: 1,
    nonce,
    issuedAt: new Date().toISOString(),
  });
  const message = siweMsg.prepareMessage();

  // Sign
  const signature = await account.signMessage({ message });
  log(`  Signed (${signature.slice(0, 20)}...)`);

  // Verify
  const verifyRes = await fetch(`${BASE_URL}/api/auth/siwe`, {
    method: "POST",
    headers: { "Content-Type": "application/json", cookie: sessionCookie },
    body: JSON.stringify({ message, signature }),
  });

  if (!verifyRes.ok) {
    const err = await verifyRes.text();
    log(`  FAILED: ${verifyRes.status} ${err}`);
    return false;
  }

  const newCookie = extractCookie(verifyRes);
  if (newCookie) sessionCookie = newCookie;
  log(`  Cookie after verify: ${sessionCookie.slice(0, 40)}...`);
  const result = await verifyRes.json();
  log(`  Authenticated: ${result.address}`);

  // Derive backup key from SIWE signature (no extra signature needed)
  const backupKey = createHash("sha256").update(signature).digest("hex");
  log(`  Backup key derived from SIWE sig (${backupKey.slice(0, 16)}...)`);

  const putRes = await fetch(`${BASE_URL}/api/auth/siwe`, {
    method: "PUT",
    headers: { "Content-Type": "application/json", cookie: sessionCookie },
    body: JSON.stringify({ backupKey }),
  });
  const putCookie = extractCookie(putRes);
  if (putCookie) sessionCookie = putCookie;
  log(`  Backup key stored: ${putRes.ok}`);

  return true;
}

async function step2_deploy(): Promise<string | null> {
  log("Step 2: Deploy Agent");

  const res = await fetch(`${BASE_URL}/api/deploy`, {
    method: "POST",
    headers: { "Content-Type": "application/json", cookie: sessionCookie },
    body: JSON.stringify({
      strategyId: "sports-basic",
      config: { fundAmountUsd: 10, riskLevel: "low" },
    }),
  });

  if (!res.ok) {
    const err = await res.text();
    log(`  FAILED: ${res.status}`);
    log(`  Response body: ${err}`);
    log(`  Cookie sent: ${sessionCookie.slice(0, 40)}...`);
    return null;
  }

  const deployCookie = extractCookie(res);
  if (deployCookie) sessionCookie = deployCookie;
  const data = await res.json() as { id: string; akashDseq: string; status: string; error?: string };

  if (data.error) {
    log(`  Deploy error: ${data.error}`);
    return null;
  }

  log(`  Deployment: ${data.id}`);
  log(`  Akash dseq: ${data.akashDseq}`);
  log(`  Status: ${data.status}`);
  return data.id;
}

async function step3_monitorSSE(deploymentId: string): Promise<boolean> {
  log("Step 3: Monitor SSE Deploy Stream");

  const url = `${BASE_URL}/api/deploy/stream?deploymentId=${deploymentId}`;
  const res = await fetch(url, {
    headers: { cookie: sessionCookie },
  });

  if (!res.ok) {
    // Non-SSE response (already terminal or JSON)
    const data = await res.json().catch(() => ({}));
    log(`  Status: ${(data as Record<string, unknown>).status ?? res.status}`);
    return (data as Record<string, unknown>).status === "active";
  }

  if (!res.body) {
    log("  No SSE body");
    return false;
  }

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  const startTime = Date.now();
  const timeout = 180_000; // 3 min

  while (Date.now() - startTime < timeout) {
    const { done, value } = await reader.read();
    if (done) break;

    const text = decoder.decode(value, { stream: true });
    const lines = text.split("\n").filter((l) => l.startsWith("data: "));

    for (const line of lines) {
      try {
        const data = JSON.parse(line.slice(6));
        log(`  SSE: ${JSON.stringify(data)}`);

        if (data.status === "active") {
          log("  Agent is ACTIVE!");
          reader.cancel();
          return true;
        }
        if (data.error) {
          log(`  Deploy FAILED: ${data.error}`);
          reader.cancel();
          return false;
        }
      } catch {}
    }
  }

  log("  SSE timeout (3 min)");
  return false;
}

async function step4_verifyDashboard(deploymentId: string): Promise<boolean> {
  log("Step 4: Verify Dashboard Data");

  const res = await fetch(`${BASE_URL}/api/agent?deploymentId=${deploymentId}`, {
    headers: { cookie: sessionCookie },
  });

  if (!res.ok) {
    log(`  FAILED: ${res.status}`);
    return false;
  }

  const data = await res.json() as {
    deployment: { status: string; config: Record<string, unknown> };
    trades: unknown[];
    messages: unknown[];
  };
  log(`  Status: ${data.deployment.status}`);
  log(`  Trades: ${data.trades.length}`);
  log(`  Messages: ${data.messages.length}`);

  // Verify secrets are NOT leaked
  const config = data.deployment.config;
  const leaked = ["_callbackToken", "_gatewayToken", "_openrouterApiKey", "_waiaasPassword", "_backupKey"].filter(
    (k) => k in config
  );
  if (leaked.length > 0) {
    log(`  SECRET LEAK DETECTED: ${leaked.join(", ")}`);
    return false;
  }
  log("  Config secrets properly filtered");

  return true;
}

async function step5_callbackSecrets(deploymentId: string): Promise<boolean> {
  log("Step 5: Verify Callback Secrets Endpoint");

  // We can't call this directly (needs CALLBACK_TOKEN), but we verify the endpoint exists
  const res = await fetch(`${BASE_URL}/api/agent/callback?deploymentId=${deploymentId}&type=secrets`, {
    headers: { Authorization: "Bearer fake-token" },
  });

  // Should get 401 (wrong token), not 404 (endpoint missing)
  log(`  Secrets endpoint: ${res.status} (expected 401)`);
  return res.status === 401;
}

async function step6_terminate(deploymentId: string): Promise<boolean> {
  log("Step 6: Terminate (with backup gate)");

  const res = await fetch(`${BASE_URL}/api/deploy/terminate`, {
    method: "POST",
    headers: { "Content-Type": "application/json", cookie: sessionCookie },
    body: JSON.stringify({ deploymentId }),
  });

  const data = await res.json() as Record<string, unknown>;
  log(`  Status: ${res.status}`);
  log(`  Response: ${JSON.stringify(data)}`);

  // 409 means backup gate triggered (no backup yet) — this is correct!
  if (res.status === 409) {
    log("  Backup gate working — no backup exists yet");
    // Force terminate for cleanup
    const forceRes = await fetch(`${BASE_URL}/api/deploy/terminate`, {
      method: "POST",
      headers: { "Content-Type": "application/json", cookie: sessionCookie },
      body: JSON.stringify({ deploymentId, forceWithoutBackup: true }),
    });
    const forceData = await forceRes.json() as Record<string, unknown>;
    log(`  Force terminate: ${forceData.status}`);
    return true;
  }

  return data.status === "terminated";
}

// Main
(async () => {
  console.log("=== A2EX Headless E2E Test ===");
  console.log(`Target: ${BASE_URL}`);
  console.log(`Wallet: ${account.address}`);
  console.log("");

  const results: Record<string, boolean> = {};

  // Step 1: Auth
  results["SIWE Auth"] = await step1_siweAuth();
  if (!results["SIWE Auth"]) {
    console.log("\n❌ Auth failed — cannot continue");
    process.exit(1);
  }

  // Step 2: Deploy
  const deploymentId = await step2_deploy();
  results["Deploy"] = deploymentId !== null;
  if (!deploymentId) {
    console.log("\n❌ Deploy failed — cannot continue");
    process.exit(1);
  }

  // Step 3: SSE Monitor
  results["SSE Stream"] = await step3_monitorSSE(deploymentId);

  // Step 4: Dashboard verification (works regardless of deploy outcome)
  results["Dashboard API"] = await step4_verifyDashboard(deploymentId);

  // Step 5: Callback secrets endpoint
  results["Secrets Endpoint"] = await step5_callbackSecrets(deploymentId);

  // Step 6: Terminate
  results["Terminate"] = await step6_terminate(deploymentId);

  // Summary
  console.log("\n=== E2E RESULTS ===");
  let allPass = true;
  for (const [name, pass] of Object.entries(results)) {
    console.log(`  ${pass ? "✅" : "❌"} ${name}`);
    if (!pass) allPass = false;
  }
  console.log(`\n${allPass ? "ALL PASSED" : "SOME FAILED"}`);
  process.exit(allPass ? 0 : 1);
})();
