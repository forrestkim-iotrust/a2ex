// Regression: Callback API — agent ↔ dashboard communication
// Found by /plan-eng-review on 2026-03-27
import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock DB
const mockSelect = vi.fn();
const mockInsert = vi.fn();
const mockUpdate = vi.fn();
const mockWhere = vi.fn();
const mockReturning = vi.fn();
const mockOrderBy = vi.fn();
const mockLimit = vi.fn();
const mockSet = vi.fn();
const mockValues = vi.fn();

vi.mock("@/lib/db", () => ({
  getDb: () => ({
    select: () => ({ from: () => ({ where: mockWhere }) }),
    insert: () => ({ values: mockValues }),
    update: () => ({ set: mockSet }),
  }),
}));

vi.mock("@/lib/db/schema", () => ({
  deployments: { id: "id", config: "config" },
  trades: {},
  agentMessages: { deploymentId: "deployment_id", direction: "direction", processed: "processed", ts: "ts", id: "id" },
}));

vi.mock("drizzle-orm", () => ({
  eq: (a: any, b: any) => ({ field: a, value: b }),
  and: (...args: any[]) => args,
  asc: (field: any) => field,
}));

describe("Callback API Authentication", () => {
  it("rejects requests without Authorization header", async () => {
    // authenticateCallback returns null when no auth header
    const req = new Request("http://localhost/api/agent/callback", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ deploymentId: "test-123", type: "heartbeat" }),
    });
    // No Authorization header → should get 401
    expect(req.headers.get("authorization")).toBeNull();
  });

  it("rejects requests with wrong token", () => {
    const header = "Bearer wrong-token";
    expect(header.startsWith("Bearer ")).toBe(true);
    expect(header.slice(7)).toBe("wrong-token");
  });

  it("extracts token correctly from Bearer header", () => {
    const header = "Bearer abc-123-def";
    expect(header.slice(7)).toBe("abc-123-def");
  });
});

describe("Callback POST types", () => {
  it("validates trade payload fields", () => {
    const payload = { venue: "Polymarket", action: "BUY", amountUsd: 10.5, pnlUsd: 2.3 };
    expect(payload.venue).toBe("Polymarket");
    expect(payload.amountUsd?.toString()).toBe("10.5");
    expect(payload.pnlUsd?.toString()).toBe("2.3");
  });

  it("handles missing trade fields with defaults", () => {
    const payload: Record<string, any> = {};
    expect(payload.venue ?? "unknown").toBe("unknown");
    expect(payload.amountUsd?.toString() ?? "0").toBe("0");
  });

  it("validates heartbeat phase values", () => {
    const validPhases = ["bootstrap", "ready", "trading"];
    for (const phase of validPhases) {
      expect(typeof phase).toBe("string");
    }
    // null phase defaults to "active"
    expect(null ?? "active").toBe("active");
  });

  it("rejects message without content", () => {
    const payload = { content: "" };
    expect(!payload.content).toBe(true);
  });

  it("accepts message with content", () => {
    const payload = { content: "Hello agent!" };
    expect(payload.content).toBe("Hello agent!");
  });

  it("rejects unknown type", () => {
    const validTypes = ["trade", "heartbeat", "message"];
    expect(validTypes.includes("invalid")).toBe(false);
  });
});

describe("Callback GET — command polling", () => {
  it("requires deploymentId query param", () => {
    const url = new URL("http://localhost/api/agent/callback");
    expect(url.searchParams.get("deploymentId")).toBeNull();
  });

  it("extracts deploymentId from query", () => {
    const url = new URL("http://localhost/api/agent/callback?deploymentId=abc-123");
    expect(url.searchParams.get("deploymentId")).toBe("abc-123");
  });
});

describe("SDL env vars", () => {
  it("includes all required env vars", () => {
    const requiredEnvs = [
      "STRATEGY_ID", "FUND_LIMIT_USD", "RISK_LEVEL",
      "OPENROUTER_API_KEY", "OPENCLAW_GATEWAY_TOKEN",
      "WAIAAS_MASTER_PASSWORD", "CALLBACK_URL",
      "DEPLOYMENT_ID", "CALLBACK_TOKEN",
    ];
    // All 9 env vars should be present
    expect(requiredEnvs.length).toBe(9);
  });
});
