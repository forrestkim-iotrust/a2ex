import { neon } from "@neondatabase/serverless";

const DEP_ID = process.argv[2];
if (!DEP_ID) { console.error("Usage: npx tsx scripts/test-gateway-chat.ts <deploymentId>"); process.exit(1); }

async function main() {
  const sql = neon(process.env.NEON_DATABASE_URL!);
  const rows = await sql`SELECT config FROM deployments WHERE id = ${DEP_ID}`;
  const config = rows[0]?.config as Record<string, any>;
  if (!config) { console.error("Deployment not found"); process.exit(1); }

  const gw = config._gatewayUrl;
  const token = config._gatewayToken;
  console.log("Gateway:", gw);
  console.log("Token:", token?.slice(0, 8) + "...");

  // Test non-streaming first
  console.log("\n=== Non-streaming test ===");
  const res1 = await fetch(`${gw}/v1/chat/completions`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ model: "openclaw/default", messages: [{ role: "user", content: "Hello" }], stream: false }),
  });
  console.log("Status:", res1.status);
  console.log("Content-Type:", res1.headers.get("content-type"));
  const body1 = await res1.text();
  console.log("Body:", body1.slice(0, 500));

  // Test streaming
  console.log("\n=== Streaming test ===");
  const res2 = await fetch(`${gw}/v1/chat/completions`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ model: "openclaw/default", messages: [{ role: "user", content: "Hello" }], stream: true }),
  });
  console.log("Status:", res2.status);
  console.log("Content-Type:", res2.headers.get("content-type"));
  if (res2.body) {
    const reader = res2.body.getReader();
    const dec = new TextDecoder();
    let chunks = 0;
    while (chunks < 20) {
      const { done, value } = await reader.read();
      if (done) break;
      console.log(`Chunk ${++chunks}:`, dec.decode(value, { stream: true }).slice(0, 200));
    }
    reader.cancel();
  }
}

main();
