import { describe, it, expect } from "vitest";
import { buildSDL, type DeployConfig } from "@/lib/akash/sdl";

function makeConfig(overrides: Partial<DeployConfig> = {}): DeployConfig {
  return {
    strategyId: "momentum",
    fundAmountUsd: 50,
    riskLevel: "medium",
    openclawGatewayToken: "gw-token-123",
    callbackUrl: "https://a2ex.xyz/api/agent/callback",
    deploymentId: "dep-001",
    callbackToken: "cb-token-789",
    ...overrides,
  };
}

describe("SDL Generation", () => {
  it("produces output containing all required env var keys", () => {
    const sdl = buildSDL(makeConfig());
    const requiredKeys = [
      "STRATEGY_ID",
      "FUND_LIMIT_USD",
      "RISK_LEVEL",
      "OPENCLAW_GATEWAY_TOKEN",
      "CALLBACK_URL",
      "DEPLOYMENT_ID",
      "CALLBACK_TOKEN",
    ];
    for (const key of requiredKeys) {
      expect(sdl).toContain(key);
    }
  });

  it("includes the correct image tag", () => {
    const sdl = buildSDL(makeConfig());
    expect(sdl).toContain("ghcr.io/forrestkim-iotrust/a2ex:sha-3585f6f");
  });

  it("includes port 18789 (gateway)", () => {
    const sdl = buildSDL(makeConfig());
    expect(sdl).toContain("18789");
  });

  it("includes version 2.0", () => {
    const sdl = buildSDL(makeConfig());
    expect(sdl).toContain("version: '2.0'");
  });

  it("includes service name a2ex-agent", () => {
    const sdl = buildSDL(makeConfig());
    expect(sdl).toContain("a2ex-agent");
  });

  it("embeds the deploymentId in env vars", () => {
    const sdl = buildSDL(makeConfig({ deploymentId: "unique-dep-xyz" }));
    expect(sdl).toContain("unique-dep-xyz");
  });

  it("embeds the callbackUrl in env vars", () => {
    const sdl = buildSDL(makeConfig({ callbackUrl: "https://custom.host/api/agent/callback" }));
    expect(sdl).toContain("https://custom.host/api/agent/callback");
  });
});

describe("SDL Sanitize", () => {
  it("strips shell metacharacters from strategyId", () => {
    const sdl = buildSDL(makeConfig({ strategyId: "test;rm -rf /" }));
    expect(sdl).not.toContain(";");
    expect(sdl).not.toContain("rm -rf");
    expect(sdl).toContain("testrm-rf");
  });

  it("strips quotes from riskLevel", () => {
    const sdl = buildSDL(makeConfig({ riskLevel: 'high$injected' }));
    // sanitize removes the $, leaving "highinjected"
    expect(sdl).toContain("highinjected");
    expect(sdl).not.toContain("$");
  });

  it("preserves valid alphanumeric + hyphen + underscore", () => {
    const sdl = buildSDL(makeConfig({ strategyId: "my_strategy-v2" }));
    expect(sdl).toContain("my_strategy-v2");
  });

  it("strips spaces from input", () => {
    const sdl = buildSDL(makeConfig({ strategyId: "has spaces here" }));
    expect(sdl).toContain("hasspaceshere");
    expect(sdl).not.toContain("has spaces");
  });
});

describe("fundAmountUsd Clamping", () => {
  it("clamps below minimum (10) to 10", () => {
    const sdl = buildSDL(makeConfig({ fundAmountUsd: 1 }));
    expect(sdl).toContain("FUND_LIMIT_USD=10");
    expect(sdl).not.toContain("FUND_LIMIT_USD=1\n");
  });

  it("clamps above maximum (1000) to 1000", () => {
    const sdl = buildSDL(makeConfig({ fundAmountUsd: 5000 }));
    expect(sdl).toContain("FUND_LIMIT_USD=1000");
    expect(sdl).not.toContain("FUND_LIMIT_USD=5000");
  });

  it("passes through valid amount unchanged", () => {
    const sdl = buildSDL(makeConfig({ fundAmountUsd: 250 }));
    expect(sdl).toContain("FUND_LIMIT_USD=250");
  });

  it("clamps exactly at min boundary", () => {
    const sdl = buildSDL(makeConfig({ fundAmountUsd: 10 }));
    expect(sdl).toContain("FUND_LIMIT_USD=10");
  });

  it("clamps exactly at max boundary", () => {
    const sdl = buildSDL(makeConfig({ fundAmountUsd: 1000 }));
    expect(sdl).toContain("FUND_LIMIT_USD=1000");
  });

  it("clamps negative values to 10", () => {
    const sdl = buildSDL(makeConfig({ fundAmountUsd: -100 }));
    expect(sdl).toContain("FUND_LIMIT_USD=10");
  });

  it("clamps zero to 10", () => {
    const sdl = buildSDL(makeConfig({ fundAmountUsd: 0 }));
    expect(sdl).toContain("FUND_LIMIT_USD=10");
  });
});

describe("SDL Port Exposure", () => {
  it("exposes port 18789 for gateway", () => {
    const sdl = buildSDL(makeConfig());
    expect(sdl).toContain("18789");
  });

  it("does NOT expose port 3100 (WAIaaS internal only)", () => {
    const sdl = buildSDL(makeConfig());
    expect(sdl).not.toContain("3100");
  });
});
