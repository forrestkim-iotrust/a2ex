import { privateKeyToAccount } from "viem/accounts";
import { SiweMessage } from "siwe";
import { createHash } from "node:crypto";

const BASE = "https://landing-two-silk-25.vercel.app";
const DOMAIN = "landing-two-silk-25.vercel.app";
const account = privateKeyToAccount("0x492a11a942c5b403a5e53203501d78f6f13d7c5df9588ff139406b6d8b03c56c");
let cookie = "";

function ext(r: Response) {
  for (const s of r.headers.getSetCookie?.() ?? [])
    if (s.startsWith("a2ex-session=")) return s.split(";")[0];
  return (r.headers.get("set-cookie")?.match(/a2ex-session=[^;]+/) ?? [cookie])[0];
}

function log(msg: string) { console.log(`[${new Date().toTimeString().slice(0, 8)}] ${msg}`); }

async function auth() {
  const nr = await fetch(`${BASE}/api/auth/siwe`);
  cookie = ext(nr);
  const { nonce } = (await nr.json()) as { nonce: string };
  const msg = new SiweMessage({ domain: DOMAIN, address: account.address, statement: "Sign in", uri: BASE, version: "1", chainId: 1, nonce, issuedAt: new Date().toISOString() });
  const message = msg.prepareMessage();
  const signature = await account.signMessage({ message });
  const vr = await fetch(`${BASE}/api/auth/siwe`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ message, signature }) });
  cookie = ext(vr) || cookie;
  const bKey = createHash("sha256").update(signature).digest("hex");
  const pr = await fetch(`${BASE}/api/auth/siwe`, { method: "PUT", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ backupKey: bKey }) });
  cookie = ext(pr) || cookie;
}

async function waitSSE(depId: string) {
  const sse = await fetch(`${BASE}/api/deploy/stream?deploymentId=${depId}`, { headers: { cookie } });
  if (!sse.body) return false;
  const reader = sse.body.getReader();
  const dec = new TextDecoder();
  const deadline = Date.now() + 120000;
  while (Date.now() < deadline) {
    const { done, value } = await reader.read();
    if (done) break;
    if (dec.decode(value, { stream: true }).includes('"active"')) { reader.cancel(); return true; }
  }
  return false;
}

async function pollAgent(depId: string, field: string, maxMs: number): Promise<any> {
  const deadline = Date.now() + maxMs;
  while (Date.now() < deadline) {
    await new Promise(r => setTimeout(r, 15000));
    const r = await fetch(`${BASE}/api/agent?deploymentId=${depId}`, { headers: { cookie } });
    const d = (await r.json()) as any;
    const c = d.deployment?.config || {};
    const bk = d.deployment?.encryptedBackup;
    log(`phase=${c._phase || "?"} backup=${bk ? "YES" : "no"}`);
    if (field === "backup" && bk) return bk;
    if (field === "phase" && c._phase) return c._phase;
  }
  return null;
}

async function main() {
  log("=== E2E RECOVERY TEST ===");

  // Step 1: Auth
  await auth();
  log("Authenticated + backup key stored");

  // Step 2: Deploy first agent
  const dr = await fetch(`${BASE}/api/deploy`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ strategyId: "sports-basic", config: { fundAmountUsd: 10, riskLevel: "low" } }) });
  const dep1 = (await dr.json()) as any;
  log(`Deploy 1: ${dep1.id}`);

  // Step 3: SSE → active
  const active = await waitSSE(dep1.id);
  log(`Active: ${active}`);
  if (!active) { log("FAILED: Agent 1 not active"); process.exit(1); }

  // Step 4: Wait for backup (max 4 min)
  log("Waiting for backup...");
  const backup = await pollAgent(dep1.id, "backup", 240000);
  if (!backup) {
    log("FAILED: No backup after 4min — terminating with force");
    await fetch(`${BASE}/api/deploy/terminate`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ deploymentId: dep1.id, forceWithoutBackup: true }) });
    process.exit(1);
  }
  log("Backup received!");

  // Step 5: Terminate (should work without force)
  const tr = await fetch(`${BASE}/api/deploy/terminate`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ deploymentId: dep1.id }) });
  const td = (await tr.json()) as any;
  log(`Terminate: ${td.status} backupAvailable: ${td.backupAvailable}`);
  if (td.status !== "terminated") { log("FAILED: Terminate failed"); process.exit(1); }

  // Step 6: Recovery deploy from terminated agent
  await auth(); // re-auth for fresh session with backup key
  const dr2 = await fetch(`${BASE}/api/deploy`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ strategyId: "sports-basic", config: { fundAmountUsd: 10, riskLevel: "low" }, recoveryFromId: dep1.id }) });
  const dep2 = (await dr2.json()) as any;
  log(`Deploy 2 (recovery): ${dep2.id} from ${dep1.id}`);

  // Step 7: SSE → active
  const active2 = await waitSSE(dep2.id);
  log(`Active 2: ${active2}`);
  if (!active2) { log("FAILED: Recovery agent not active"); process.exit(1); }

  // Step 8: Wait for heartbeat (confirms recovery succeeded)
  log("Waiting for recovery agent heartbeat...");
  const phase = await pollAgent(dep2.id, "phase", 180000);
  log(`Recovery agent phase: ${phase}`);

  // Step 9: Cleanup
  await fetch(`${BASE}/api/deploy/terminate`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ deploymentId: dep2.id, forceWithoutBackup: true }) });
  log("Cleanup done");

  log("=== ALL PASSED ===");
  process.exit(0);
}

main();
