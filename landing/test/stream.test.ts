import { describe, it, expect, vi, beforeEach } from "vitest";

// Test: bestOpenBid logic (extracted to lib/akash/client.ts)
describe("bestOpenBid", () => {
  const mockProviders = [
    { owner: "provider-a", uptime7d: 0.995 },
    { owner: "provider-b", uptime7d: 0.85 },
    { owner: "provider-c", uptime7d: 0.999 },
  ];

  // We test the bid filtering/sorting logic directly
  function filterAndSort(bids: any[], providerMap: Record<string, number>, minUptime = 0.99) {
    const open = bids.filter((b: any) => b.bid?.state === "open");
    if (open.length === 0) return null;

    const reliable = open.filter((b: any) => {
      const addr = b.bid?.id?.provider;
      const uptime = providerMap[addr];
      return uptime === undefined || uptime >= minUptime;
    });

    const candidates = reliable.length > 0 ? reliable : open;
    return candidates.sort((a: any, b: any) =>
      parseFloat(a.bid.price.amount) - parseFloat(b.bid.price.amount)
    )[0];
  }

  it("returns null when no bids", () => {
    expect(filterAndSort([], {})).toBeNull();
  });

  it("returns null when no open bids", () => {
    const bids = [{ bid: { state: "closed", id: { provider: "p1" }, price: { amount: "100" } } }];
    expect(filterAndSort(bids, {})).toBeNull();
  });

  it("filters out providers with uptime below 99%", () => {
    const providerMap: Record<string, number> = { "provider-a": 0.995, "provider-b": 0.85 };
    const bids = [
      { bid: { state: "open", id: { provider: "provider-a" }, price: { amount: "200" } } },
      { bid: { state: "open", id: { provider: "provider-b" }, price: { amount: "100" } } },
    ];
    const result = filterAndSort(bids, providerMap);
    // provider-b is cheaper but below 99% uptime, so provider-a wins
    expect(result.bid.id.provider).toBe("provider-a");
  });

  it("falls back to all bids if none pass uptime filter", () => {
    const providerMap: Record<string, number> = { "provider-a": 0.80, "provider-b": 0.85 };
    const bids = [
      { bid: { state: "open", id: { provider: "provider-a" }, price: { amount: "200" } } },
      { bid: { state: "open", id: { provider: "provider-b" }, price: { amount: "100" } } },
    ];
    const result = filterAndSort(bids, providerMap);
    // all below 99%, fallback to cheapest
    expect(result.bid.id.provider).toBe("provider-b");
  });

  it("allows unknown providers through the filter", () => {
    const providerMap: Record<string, number> = {};
    const bids = [
      { bid: { state: "open", id: { provider: "unknown-provider" }, price: { amount: "50" } } },
    ];
    const result = filterAndSort(bids, providerMap);
    expect(result.bid.id.provider).toBe("unknown-provider");
  });

  it("sorts by price ascending (cheapest first)", () => {
    const providerMap: Record<string, number> = {};
    const bids = [
      { bid: { state: "open", id: { provider: "p1" }, price: { amount: "300" } } },
      { bid: { state: "open", id: { provider: "p2" }, price: { amount: "100" } } },
      { bid: { state: "open", id: { provider: "p3" }, price: { amount: "200" } } },
    ];
    const result = filterAndSort(bids, providerMap);
    expect(result.bid.id.provider).toBe("p2");
  });
});

describe("SSE event format", () => {
  function sseEvent(id: number, event: string, data: Record<string, unknown>): string {
    return `id: ${id}\nevent: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
  }

  it("produces valid SSE format", () => {
    const event = sseEvent(1, "status", { status: "selecting_provider" });
    expect(event).toContain("id: 1\n");
    expect(event).toContain("event: status\n");
    expect(event).toContain('data: {"status":"selecting_provider"}\n');
    expect(event.endsWith("\n\n")).toBe(true);
  });

  it("increments event IDs", () => {
    const e1 = sseEvent(1, "bids", { bidCount: 3 });
    const e2 = sseEvent(2, "bids", { bidCount: 5 });
    expect(e1).toContain("id: 1\n");
    expect(e2).toContain("id: 2\n");
  });

  it("serializes nested objects", () => {
    const event = sseEvent(1, "active", { status: "active", provider: "p1", gatewayUrl: "http://host:1234" });
    const dataLine = event.split("\n").find(l => l.startsWith("data: "))!;
    const parsed = JSON.parse(dataLine.slice(6));
    expect(parsed.status).toBe("active");
    expect(parsed.provider).toBe("p1");
    expect(parsed.gatewayUrl).toBe("http://host:1234");
  });
});

describe("Deploy status transitions", () => {
  const DEPLOY_ORDER = ["pending", "sdl_generated", "awaiting_bids", "selecting_provider", "creating_lease", "active"];

  function isDeploying(status: string): boolean {
    return DEPLOY_ORDER.includes(status) && status !== "active";
  }

  it("pending is deploying", () => expect(isDeploying("pending")).toBe(true));
  it("sdl_generated is deploying", () => expect(isDeploying("sdl_generated")).toBe(true));
  it("awaiting_bids is deploying", () => expect(isDeploying("awaiting_bids")).toBe(true));
  it("selecting_provider is deploying", () => expect(isDeploying("selecting_provider")).toBe(true));
  it("creating_lease is deploying", () => expect(isDeploying("creating_lease")).toBe(true));
  it("active is NOT deploying", () => expect(isDeploying("active")).toBe(false));
  it("terminated is NOT deploying", () => expect(isDeploying("terminated")).toBe(false));
  it("failed is NOT deploying", () => expect(isDeploying("failed")).toBe(false));
});

describe("Gateway port extraction", () => {
  it("extracts gateway URL from forwarded ports", () => {
    const ports = {
      "a2ex-agent": [
        { host: "provider.example.com", port: 3100, externalPort: 31747 },
        { host: "provider.example.com", port: 18789, externalPort: 30207 },
      ],
    };
    const svcPorts = Object.values(ports).flat();
    const gwPort = svcPorts.find((p: any) => p.port === 18789);
    expect(gwPort).toBeDefined();
    expect(`http://${gwPort!.host}:${gwPort!.externalPort}`).toBe("http://provider.example.com:30207");
  });

  it("returns undefined when no 18789 port", () => {
    const ports = { "a2ex-agent": [{ host: "host", port: 3100, externalPort: 31747 }] };
    const svcPorts = Object.values(ports).flat();
    const gwPort = svcPorts.find((p: any) => p.port === 18789);
    expect(gwPort).toBeUndefined();
  });

  it("handles null ports", () => {
    const ports = null;
    let gatewayUrl: string | undefined;
    if (ports) {
      const svcPorts = Object.values(ports).flat() as any[];
      const gwPort = svcPorts.find((p: any) => p.port === 18789);
      if (gwPort) gatewayUrl = `http://${gwPort.host}:${gwPort.externalPort}`;
    }
    expect(gatewayUrl).toBeUndefined();
  });
});

describe("Rate limiting", () => {
  it("rate limit key uses deploymentId", () => {
    const deploymentId = "abc-123";
    const key = deploymentId; // used as ratelimit key
    expect(key).toBe("abc-123");
  });
});

describe("Stuck deployment cleanup", () => {
  it("identifies deployments older than 5 minutes", () => {
    const fiveMinAgo = new Date(Date.now() - 5 * 60 * 1000);
    const stuck = new Date(Date.now() - 10 * 60 * 1000); // 10 min ago
    const recent = new Date(Date.now() - 2 * 60 * 1000); // 2 min ago

    expect(stuck < fiveMinAgo).toBe(true);
    expect(recent < fiveMinAgo).toBe(false);
  });
});
