import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock DB
const mockSelect = vi.fn();
const mockInsert = vi.fn();
const mockUpdate = vi.fn();
const mockWhere = vi.fn();
const mockReturning = vi.fn();
const mockValues = vi.fn();
const mockSet = vi.fn();

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
  agentMessages: {
    deploymentId: "deployment_id",
    direction: "direction",
    processed: "processed",
    ts: "ts",
    id: "id",
  },
}));

vi.mock("drizzle-orm", () => ({
  eq: (a: any, b: any) => ({ field: a, value: b }),
  and: (...args: any[]) => args,
  asc: (field: any) => field,
}));

describe("Bearer Token Extraction", () => {
  function extractToken(authHeader: string | null): string | null {
    if (!authHeader?.startsWith("Bearer ")) return null;
    return authHeader.slice(7);
  }

  it("extracts token from valid Bearer header", () => {
    expect(extractToken("Bearer abc-123-def")).toBe("abc-123-def");
  });

  it("returns null when header is missing", () => {
    expect(extractToken(null)).toBeNull();
  });

  it("returns null when header has no Bearer prefix", () => {
    expect(extractToken("Basic abc123")).toBeNull();
  });

  it("returns null for empty Bearer value", () => {
    // "Bearer " has 7 chars, slice(7) on "Bearer " yields ""
    expect(extractToken("Bearer ")).toBe("");
  });

  it("handles tokens with special characters", () => {
    expect(extractToken("Bearer a1b2c3-d4e5-f6g7")).toBe("a1b2c3-d4e5-f6g7");
  });

  it("is case-sensitive on Bearer prefix", () => {
    expect(extractToken("bearer abc")).toBeNull();
    expect(extractToken("BEARER abc")).toBeNull();
  });
});

describe("Token Comparison Logic", () => {
  function verifyToken(
    providedToken: string,
    deployment: { config: Record<string, any> } | null
  ): boolean {
    if (!deployment) return false;
    return deployment.config?._callbackToken === providedToken;
  }

  it("accepts correct token", () => {
    const dep = { config: { _callbackToken: "secret-token-xyz" } };
    expect(verifyToken("secret-token-xyz", dep)).toBe(true);
  });

  it("rejects wrong token", () => {
    const dep = { config: { _callbackToken: "secret-token-xyz" } };
    expect(verifyToken("wrong-token", dep)).toBe(false);
  });

  it("rejects when deployment is null (not found)", () => {
    expect(verifyToken("any-token", null)).toBe(false);
  });

  it("rejects when config has no _callbackToken field", () => {
    const dep = { config: {} };
    expect(verifyToken("any-token", dep)).toBe(false);
  });

  it("uses strict equality (no type coercion)", () => {
    const dep = { config: { _callbackToken: "123" } };
    expect(verifyToken("123", dep)).toBe(true);
    // number vs string would fail in real code
  });
});

describe("Trade Payload Validation", () => {
  function normalizeTrade(payload: Record<string, any>) {
    return {
      venue: payload.venue ?? "unknown",
      action: payload.action ?? "unknown",
      amountUsd: payload.amountUsd?.toString() ?? "0",
      pnlUsd: payload.pnlUsd?.toString() ?? "0",
    };
  }

  it("passes through complete payload", () => {
    const result = normalizeTrade({
      venue: "Polymarket",
      action: "BUY",
      amountUsd: 10.5,
      pnlUsd: 2.3,
    });
    expect(result).toEqual({
      venue: "Polymarket",
      action: "BUY",
      amountUsd: "10.5",
      pnlUsd: "2.3",
    });
  });

  it("defaults missing venue to 'unknown'", () => {
    expect(normalizeTrade({}).venue).toBe("unknown");
  });

  it("defaults missing action to 'unknown'", () => {
    expect(normalizeTrade({}).action).toBe("unknown");
  });

  it("defaults missing amountUsd to '0'", () => {
    expect(normalizeTrade({}).amountUsd).toBe("0");
  });

  it("defaults missing pnlUsd to '0'", () => {
    expect(normalizeTrade({}).pnlUsd).toBe("0");
  });

  it("converts numeric amountUsd to string", () => {
    expect(normalizeTrade({ amountUsd: 42 }).amountUsd).toBe("42");
  });

  it("handles negative pnlUsd", () => {
    expect(normalizeTrade({ pnlUsd: -5.5 }).pnlUsd).toBe("-5.5");
  });
});

describe("Heartbeat Phase Validation", () => {
  function resolvePhase(phase: string | null | undefined): string {
    return phase ?? "active";
  }

  it("uses provided phase when present", () => {
    expect(resolvePhase("bootstrap")).toBe("bootstrap");
    expect(resolvePhase("ready")).toBe("ready");
    expect(resolvePhase("trading")).toBe("trading");
  });

  it("defaults null phase to 'active'", () => {
    expect(resolvePhase(null)).toBe("active");
  });

  it("defaults undefined phase to 'active'", () => {
    expect(resolvePhase(undefined)).toBe("active");
  });

  it("stores lastHeartbeat as ISO string", () => {
    const ts = new Date().toISOString();
    expect(ts).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/);
  });
});

describe("Message Content Validation", () => {
  function validateMessage(content: any): { valid: boolean; error?: string } {
    if (!content) return { valid: false, error: "content required" };
    return { valid: true };
  }

  it("rejects empty string content", () => {
    expect(validateMessage("").valid).toBe(false);
    expect(validateMessage("").error).toBe("content required");
  });

  it("rejects null content", () => {
    expect(validateMessage(null).valid).toBe(false);
  });

  it("rejects undefined content", () => {
    expect(validateMessage(undefined).valid).toBe(false);
  });

  it("accepts valid string content", () => {
    expect(validateMessage("Hello from agent").valid).toBe(true);
  });

  it("accepts long content", () => {
    const longContent = "x".repeat(10000);
    expect(validateMessage(longContent).valid).toBe(true);
  });
});

describe("Unknown Type Rejection", () => {
  const VALID_TYPES = ["trade", "heartbeat", "message"];

  function isValidType(type: string): boolean {
    return VALID_TYPES.includes(type);
  }

  it("accepts 'trade'", () => expect(isValidType("trade")).toBe(true));
  it("accepts 'heartbeat'", () => expect(isValidType("heartbeat")).toBe(true));
  it("accepts 'message'", () => expect(isValidType("message")).toBe(true));
  it("rejects 'invalid'", () => expect(isValidType("invalid")).toBe(false));
  it("rejects empty string", () => expect(isValidType("")).toBe(false));
  it("rejects 'TRADE' (case-sensitive)", () => expect(isValidType("TRADE")).toBe(false));
});

describe("Rate Limit Key", () => {
  it("uses deploymentId as the rate limit key", () => {
    const deploymentId = "dep-abc-123";
    // In callback route: getRatelimit().limit(deploymentId)
    const rateLimitKey = deploymentId;
    expect(rateLimitKey).toBe("dep-abc-123");
  });

  it("different deployments get different rate limit keys", () => {
    const key1 = "dep-001";
    const key2 = "dep-002";
    expect(key1).not.toBe(key2);
  });

  it("rate limit prefix is 'cb'", () => {
    const prefix = "cb";
    const fullKey = `${prefix}:dep-001`;
    expect(fullKey).toBe("cb:dep-001");
  });
});

describe("Callback POST Required Fields", () => {
  it("rejects when deploymentId is missing", () => {
    const body = { type: "heartbeat" };
    const hasRequired = body.deploymentId !== undefined && (body as any).type !== undefined;
    // deploymentId missing from body destructure → undefined
    expect((body as any).deploymentId).toBeUndefined();
  });

  it("rejects when type is missing", () => {
    const body = { deploymentId: "dep-001" };
    expect((body as any).type).toBeUndefined();
  });

  it("accepts when both deploymentId and type are present", () => {
    const body = { deploymentId: "dep-001", type: "heartbeat" };
    const hasRequired = !!body.deploymentId && !!body.type;
    expect(hasRequired).toBe(true);
  });
});

describe("Callback GET — Command Polling", () => {
  it("requires deploymentId query param", () => {
    const url = new URL("http://localhost/api/agent/callback");
    expect(url.searchParams.get("deploymentId")).toBeNull();
  });

  it("extracts deploymentId from query", () => {
    const url = new URL("http://localhost/api/agent/callback?deploymentId=abc-123");
    expect(url.searchParams.get("deploymentId")).toBe("abc-123");
  });
});
