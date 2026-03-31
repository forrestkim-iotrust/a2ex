import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments, agentMessages } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { log } from "@/lib/log";
import WebSocket from "ws";
import { randomUUID } from "node:crypto";

export const dynamic = "force-dynamic";
export const maxDuration = 300;

/**
 * POST /api/agent/chat — SSE streaming chat with the agent.
 *
 * 1. Authenticates user, verifies deployment ownership
 * 2. Opens WS to OpenClaw gateway on the Akash container
 * 3. Sends message via sessions.send
 * 4. Subscribes to session messages
 * 5. Streams agent response tokens back as SSE
 */
export async function POST(req: NextRequest) {
  const auth = await requireAuth();
  if (auth instanceof NextResponse) return auth;

  const { deploymentId, content } = await req.json();

  if (!content || content.length > 2000) {
    return NextResponse.json({ error: "Message must be 1-2000 characters" }, { status: 400 });
  }

  const db = getDb();
  const [deployment] = await db.select().from(deployments)
    .where(and(eq(deployments.id, deploymentId), eq(deployments.userAddress, auth.userAddress!)));

  if (!deployment) {
    return NextResponse.json({ error: "Not found" }, { status: 404 });
  }

  const config = deployment.config as Record<string, any>;
  const gatewayUrl = config?._gatewayUrl as string | undefined;
  const gatewayToken = config?._gatewayToken as string | undefined;

  if (!gatewayUrl || !gatewayToken) {
    return NextResponse.json({ error: "Agent not ready" }, { status: 503 });
  }

  // Save user message to DB
  await db.insert(agentMessages).values({
    deploymentId,
    direction: "user_to_agent",
    content,
  });

  log("chat.started", { deploymentId, messageLength: content.length });

  // Parse gateway WS URL from HTTP URL
  const wsUrl = gatewayUrl.replace(/^http/, "ws");

  const encoder = new TextEncoder();
  let wsClosed = false;

  const stream = new ReadableStream({
    start(controller) {
      const send = (event: string, data: string) => {
        try {
          controller.enqueue(encoder.encode(`event: ${event}\ndata: ${data}\n\n`));
        } catch { /* stream may be closed */ }
      };

      send("status", JSON.stringify({ status: "connecting" }));

      // Connect to gateway WS
      const ws = new WebSocket(wsUrl);
      let connected = false;
      let subscribed = false;
      let agentResponseText = "";

      const cleanup = () => {
        if (!wsClosed) {
          wsClosed = true;
          try { ws.close(); } catch {}
        }
      };

      const timeout = setTimeout(() => {
        send("error", JSON.stringify({ error: "Timeout" }));
        try { controller.close(); } catch {}
        cleanup();
      }, 280000); // 280s (under maxDuration 300s)

      ws.on("open", () => {
        // Gateway connect handshake
        const connectId = randomUUID();
        ws.send(JSON.stringify({
          type: "req",
          id: connectId,
          method: "connect",
          params: {
            minProtocol: "2026.1.0",
            maxProtocol: "2026.3.0",
            client: { id: randomUUID(), version: "1.0.0", platform: "linux", mode: "operator" },
            role: "operator",
            scopes: ["operator.read", "operator.write"],
            auth: { token: gatewayToken },
          },
        }));
      });

      ws.on("message", (data: Buffer) => {
        try {
          const frame = JSON.parse(data.toString());

          // Handle connect response
          if (frame.type === "res" && frame.ok && !connected) {
            connected = true;
            send("status", JSON.stringify({ status: "connected" }));

            // Subscribe to session messages first
            const subId = randomUUID();
            ws.send(JSON.stringify({
              type: "req",
              id: subId,
              method: "sessions.messages.subscribe",
              params: { key: "main" },
            }));

            // Send the user's message
            const sendId = randomUUID();
            ws.send(JSON.stringify({
              type: "req",
              id: sendId,
              method: "sessions.send",
              params: {
                key: "main",
                message: content,
                thinking: "medium",
                timeoutMs: 120000,
              },
            }));

            send("status", JSON.stringify({ status: "thinking" }));
            subscribed = true;
          }

          // Handle session message events (token streaming)
          if (frame.type === "event" && subscribed) {
            const payload = frame.payload || {};

            // Agent text token
            if (payload.type === "text" || payload.type === "assistant_text" || payload.delta) {
              const token = payload.delta || payload.text || payload.content || "";
              if (token) {
                agentResponseText += token;
                send("token", JSON.stringify({ token }));
              }
            }

            // Message complete
            if (payload.type === "message_complete" || payload.type === "done" ||
                (payload.role === "assistant" && payload.complete)) {
              // Save agent response to DB
              if (agentResponseText) {
                db.insert(agentMessages).values({
                  deploymentId,
                  direction: "agent_to_user",
                  content: agentResponseText,
                }).then(() => {}).catch(() => {});
              }

              send("done", JSON.stringify({ fullText: agentResponseText }));
              clearTimeout(timeout);
              try { controller.close(); } catch {}
              cleanup();
            }
          }

          // Handle errors
          if (frame.type === "res" && !frame.ok && connected) {
            send("error", JSON.stringify({ error: frame.error?.message || "Agent error" }));
            clearTimeout(timeout);
            try { controller.close(); } catch {}
            cleanup();
          }
        } catch {}
      });

      ws.on("error", (err: Error) => {
        log("chat.ws_error", { deploymentId, error: err.message });
        send("error", JSON.stringify({ error: "Connection failed" }));
        clearTimeout(timeout);
        try { controller.close(); } catch {}
        cleanup();
      });

      ws.on("close", () => {
        clearTimeout(timeout);
        if (!wsClosed) {
          // Save whatever we have
          if (agentResponseText) {
            db.insert(agentMessages).values({
              deploymentId,
              direction: "agent_to_user",
              content: agentResponseText,
            }).then(() => {}).catch(() => {});
          }
          send("done", JSON.stringify({ fullText: agentResponseText }));
          try { controller.close(); } catch {}
        }
        wsClosed = true;
      });
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}
