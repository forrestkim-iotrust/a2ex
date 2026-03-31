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

async function main() {
  // Auth
  const nr = await fetch(`${BASE}/api/auth/siwe`);
  cookie = ext(nr);
  const { nonce } = (await nr.json()) as { nonce: string };
  const msg = new SiweMessage({ domain: DOMAIN, address: account.address, statement: "Sign in", uri: BASE, version: "1", chainId: 1, nonce, issuedAt: new Date().toISOString() });
  const message = msg.prepareMessage();
  const signature = await account.signMessage({ message });
  const vr = await fetch(`${BASE}/api/auth/siwe`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ message, signature }) });
  cookie = ext(vr) || cookie;

  // Backup key
  const bSig = await account.signMessage({ message: "a2ex backup key\n\nSigning creates your encrypted backup key.\nNo transaction will be sent." });
  const bKey = createHash("sha256").update(bSig).digest("hex");
  const pr = await fetch(`${BASE}/api/auth/siwe`, { method: "PUT", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ backupKey: bKey }) });
  cookie = ext(pr) || cookie;
  console.log("Auth + backup key done");

  // Deploy
  const dr = await fetch(`${BASE}/api/deploy`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ strategyId: "sports-basic", config: { fundAmountUsd: 10, riskLevel: "low" } }) });
  const dep = (await dr.json()) as { id: string; akashDseq: string };
  console.log("Deployed:", dep.id);

  // SSE → active
  const sse = await fetch(`${BASE}/api/deploy/stream?deploymentId=${dep.id}`, { headers: { cookie } });
  if (sse.body) {
    const reader = sse.body.getReader();
    const dec = new TextDecoder();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      const t = dec.decode(value, { stream: true });
      if (t.includes('"active"')) { console.log("Active!"); reader.cancel(); break; }
    }
  }

  // Poll for backup — 20s intervals, 5 min max
  for (let i = 0; i < 15; i++) {
    await new Promise((r) => setTimeout(r, 20000));
    const ar = await fetch(`${BASE}/api/agent?deploymentId=${dep.id}`, { headers: { cookie } });
    const ad = (await ar.json()) as any;
    const c = ad.deployment?.config || {};
    const bk = ad.deployment?.encryptedBackup;
    console.log(`[${new Date().toTimeString().slice(0, 8)}] phase=${c._phase || "?"} backup=${bk ? `YES(${bk.length}B)` : "no"} lastBackup=${c._lastBackupAt?.slice(11, 19) || "none"}`);

    if (bk) {
      console.log("BACKUP UPLOADED!");
      const tr = await fetch(`${BASE}/api/deploy/terminate`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ deploymentId: dep.id }) });
      const td = (await tr.json()) as any;
      console.log("Terminate (no force):", td.status, "backupAvailable:", td.backupAvailable);
      process.exit(0);
    }
  }

  console.log("No backup after 5min — force terminating");
  await fetch(`${BASE}/api/deploy/terminate`, { method: "POST", headers: { "Content-Type": "application/json", cookie }, body: JSON.stringify({ deploymentId: dep.id, forceWithoutBackup: true }) });
  process.exit(1);
}

main();
