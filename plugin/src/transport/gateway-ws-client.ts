/**
 * OpenClaw Gateway WebSocket client.
 * Sends messages to the agent's session via the gateway protocol.
 */

import { randomUUID } from "node:crypto";

interface GatewayFrame {
  type: "req" | "res" | "event";
  id?: string;
  method?: string;
  params?: Record<string, unknown>;
  ok?: boolean;
  payload?: Record<string, unknown>;
  error?: { code: string; message: string };
  event?: string;
}

export interface GatewayWsClient {
  sendSessionMessage(message: string): Promise<string | null>;
  close(): void;
}

export async function createGatewayWsClient(
  port: number,
  token: string,
): Promise<GatewayWsClient | null> {
  const WebSocket = (await import("ws")).default;

  return new Promise((resolve) => {
    const url = `ws://127.0.0.1:${port}`;
    let ws: InstanceType<typeof WebSocket>;
    let connected = false;
    const pending = new Map<string, { resolve: (v: any) => void; reject: (e: any) => void }>();

    try {
      ws = new WebSocket(url);
    } catch {
      resolve(null);
      return;
    }

    const timeout = setTimeout(() => {
      if (!connected) { ws.close(); resolve(null); }
    }, 10000);

    ws.on("open", () => {
      // Connect handshake
      const connectId = randomUUID();
      const connectFrame: GatewayFrame = {
        type: "req",
        id: connectId,
        method: "connect",
        params: {
          minProtocol: "2026.1.0",
          maxProtocol: "2026.3.0",
          client: { id: randomUUID(), version: "1.0.0", platform: "linux", mode: "operator" },
          role: "operator",
          scopes: ["operator.read", "operator.write"],
          auth: { token },
        },
      };
      ws.send(JSON.stringify(connectFrame));

      pending.set(connectId, {
        resolve: () => {
          connected = true;
          clearTimeout(timeout);
          resolve(client);
        },
        reject: () => { ws.close(); resolve(null); },
      });
    });

    ws.on("message", (data: Buffer) => {
      try {
        const frame: GatewayFrame = JSON.parse(data.toString());
        if (frame.type === "res" && frame.id) {
          const p = pending.get(frame.id);
          if (p) {
            pending.delete(frame.id);
            if (frame.ok) p.resolve(frame.payload);
            else p.reject(frame.error);
          }
        }
      } catch {}
    });

    ws.on("error", () => { clearTimeout(timeout); resolve(null); });
    ws.on("close", () => { connected = false; });

    const client: GatewayWsClient = {
      async sendSessionMessage(message: string): Promise<string | null> {
        if (!connected) return null;
        const id = randomUUID();
        const frame: GatewayFrame = {
          type: "req",
          id,
          method: "sessions.send",
          params: {
            key: "main",
            message,
            thinking: "medium",
            timeoutMs: 120000,
          },
        };
        return new Promise((res, rej) => {
          pending.set(id, {
            resolve: (payload: any) => res(payload?.runId ?? "ok"),
            reject: () => res(null),
          });
          ws.send(JSON.stringify(frame));
          setTimeout(() => { pending.delete(id); res(null); }, 120000);
        });
      },
      close() {
        connected = false;
        try { ws.close(); } catch {}
      },
    };
  });
}
