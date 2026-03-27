const AKASH_API = "https://console-api.akash.network";

async function akashFetch(path: string, options: RequestInit = {}) {
  const res = await fetch(`${AKASH_API}${path}`, {
    ...options,
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

export async function createAkashDeployment(sdl: string) {
  return akashFetch("/v1/deployments", {
    method: "POST",
    body: JSON.stringify({ data: { sdl, deposit: 500000 } }), // 0.5 AKT deposit
  });
}

export async function closeAkashDeployment(dseq: string) {
  return akashFetch(`/v1/deployments/${dseq}`, { method: "DELETE" });
}

export async function getAkashBids(dseq: string) {
  return akashFetch(`/v1/bids/${dseq}`);
}

export async function createAkashLease(dseq: string, provider: string, gseq: number, oseq: number) {
  return akashFetch("/v1/leases", {
    method: "POST",
    body: JSON.stringify({ data: { dseq, provider, gseq, oseq } }),
  });
}
