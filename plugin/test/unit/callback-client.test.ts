import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createCallbackClient } from "../../src/transport/callback-client.js";

describe("createCallbackClient", () => {
  const originalEnv = { ...process.env };

  beforeEach(() => {
    process.env = { ...originalEnv };
  });

  afterEach(() => {
    process.env = originalEnv;
    vi.restoreAllMocks();
  });

  it("returns noop client when env vars missing", () => {
    delete process.env.CALLBACK_URL;
    delete process.env.CALLBACK_TOKEN;
    delete process.env.DEPLOYMENT_ID;

    const client = createCallbackClient();
    expect(client.enabled).toBe(false);
  });

  it("returns noop client when only partial env vars set", () => {
    process.env.CALLBACK_URL = "http://localhost:3000/api/agent/callback";
    delete process.env.CALLBACK_TOKEN;
    delete process.env.DEPLOYMENT_ID;

    const client = createCallbackClient();
    expect(client.enabled).toBe(false);
  });

  it("returns enabled client when all env vars set", () => {
    process.env.CALLBACK_URL = "http://localhost:3000/api/agent/callback";
    process.env.CALLBACK_TOKEN = "test-token";
    process.env.DEPLOYMENT_ID = "test-deploy-id";

    const client = createCallbackClient();
    expect(client.enabled).toBe(true);
  });

  it("noop heartbeat does not throw", async () => {
    delete process.env.CALLBACK_URL;
    const client = createCallbackClient();
    await expect(client.heartbeat("bootstrap")).resolves.toBeUndefined();
  });

  it("noop pollCommands returns empty array", async () => {
    delete process.env.CALLBACK_URL;
    const client = createCallbackClient();
    const commands = await client.pollCommands();
    expect(commands).toEqual([]);
  });
});

describe("callback HTTP client", () => {
  const originalEnv = { ...process.env };

  beforeEach(() => {
    process.env = {
      ...originalEnv,
      CALLBACK_URL: "http://localhost:9999/api/agent/callback",
      CALLBACK_TOKEN: "test-token-123",
      DEPLOYMENT_ID: "deploy-abc",
    };
  });

  afterEach(() => {
    process.env = originalEnv;
    vi.restoreAllMocks();
  });

  it("heartbeat sends POST with correct payload", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), { status: 200 })
    );

    const client = createCallbackClient();
    await client.heartbeat("ready");

    expect(fetchSpy).toHaveBeenCalledTimes(1);
    const [url, opts] = fetchSpy.mock.calls[0];
    expect(url).toBe("http://localhost:9999/api/agent/callback");
    expect(opts.method).toBe("POST");
    expect(opts.headers["Authorization"]).toBe("Bearer test-token-123");

    const body = JSON.parse(opts.body);
    expect(body.deploymentId).toBe("deploy-abc");
    expect(body.type).toBe("heartbeat");
    expect(body.phase).toBe("ready");
  });

  it("reportTrade sends trade data", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), { status: 200 })
    );

    const client = createCallbackClient();
    await client.reportTrade({ venue: "Polymarket", action: "BUY", amountUsd: 10, pnlUsd: 2.5 });

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.type).toBe("trade");
    expect(body.venue).toBe("Polymarket");
    expect(body.amountUsd).toBe(10);
  });

  it("sendMessage sends message content", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), { status: 200 })
    );

    const client = createCallbackClient();
    await client.sendMessage("Hello from agent");

    const body = JSON.parse(fetchSpy.mock.calls[0][1].body);
    expect(body.type).toBe("message");
    expect(body.content).toBe("Hello from agent");
  });

  it("pollCommands returns command content array", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({
        commands: [
          { content: "show portfolio", direction: "user_to_agent" },
          { content: "buy more", direction: "user_to_agent" },
        ],
      }), { status: 200 })
    );

    const client = createCallbackClient();
    const commands = await client.pollCommands();
    expect(commands).toEqual(["show portfolio", "buy more"]);
  });

  it("pollCommands returns empty on network error", async () => {
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new Error("network down"));

    const client = createCallbackClient();
    const commands = await client.pollCommands();
    expect(commands).toEqual([]);
  });

  it("heartbeat retries on network error", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch")
      .mockRejectedValueOnce(new Error("timeout"))
      .mockResolvedValue(new Response(JSON.stringify({ ok: true }), { status: 200 }));

    const client = createCallbackClient();
    await client.heartbeat("trading");

    // First call fails, second succeeds
    expect(fetchSpy).toHaveBeenCalledTimes(2);
  });

  it("heartbeat handles 429 rate limit", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(new Response("rate limited", { status: 429 }))
      .mockResolvedValue(new Response(JSON.stringify({ ok: true }), { status: 200 }));

    const client = createCallbackClient();
    await client.heartbeat("bootstrap");

    expect(fetchSpy).toHaveBeenCalledTimes(2);
  });
});
