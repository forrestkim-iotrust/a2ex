const AKASH_API = "https://console-api.akash.network";

async function akashFetch(path: string, options: RequestInit = {}) {
  const res = await fetch(`${AKASH_API}${path}`, {
    ...options,
    cache: "no-store",
    headers: {
      "x-api-key": process.env.AKASH_CONSOLE_API_KEY!,
      "Content-Type": "application/json",
      ...options.headers,
    },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`Akash API ${res.status}: ${body}`);
  }
  return res.json();
}

export async function getAkashBalance() {
  return akashFetch("/v1/balances");
}

export async function listAkashDeployments() {
  return akashFetch("/v1/deployments");
}

export async function getAkashProviders(): Promise<any[]> {
  const result = await akashFetch("/v1/providers");
  return Array.isArray(result) ? result : [];
}

export async function createAkashDeployment(sdl: string, depositUsd = 5) {
  return akashFetch("/v1/deployments", {
    method: "POST",
    body: JSON.stringify({ data: { sdl, deposit: depositUsd } }),
  });
}

export async function closeAkashDeployment(dseq: string) {
  return akashFetch(`/v1/deployments/${dseq}`, { method: "DELETE" });
}

export async function getAkashBids(dseq: string) {
  return akashFetch(`/v1/bids?dseq=${dseq}`);
}

export async function createAkashLease(
  dseq: string,
  provider: string,
  gseq: number,
  oseq: number,
  manifest: string,
) {
  return akashFetch("/v1/leases", {
    method: "POST",
    body: JSON.stringify({
      manifest,
      leases: [{ dseq, gseq, oseq, provider }],
    }),
  });
}

const MIN_UPTIME = 0.99;

export async function bestOpenBid(bids: any[]): Promise<any | null> {
  const open = bids.filter((b: any) => b.bid?.state === "open");
  if (open.length === 0) return null;

  let providerMap: Record<string, number> = {};
  try {
    const providers = await getAkashProviders();
    for (const p of providers) {
      if (p.owner && p.uptime7d != null) providerMap[p.owner] = p.uptime7d;
    }
  } catch { /* fallback to no filter */ }

  const reliable = open.filter((b: any) => {
    const addr = b.bid?.id?.provider;
    const uptime = providerMap[addr];
    return uptime === undefined || uptime >= MIN_UPTIME;
  });

  const candidates = reliable.length > 0 ? reliable : open;
  return candidates.sort((a: any, b: any) =>
    parseFloat(a.bid.price.amount) - parseFloat(b.bid.price.amount)
  )[0];
}
