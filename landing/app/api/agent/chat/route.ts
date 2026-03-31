import { NextRequest, NextResponse } from "next/server";
import { requireAuth } from "@/lib/auth/middleware";
import { getDb } from "@/lib/db";
import { deployments, agentMessages } from "@/lib/db/schema";
import { eq, and } from "drizzle-orm";
import { log } from "@/lib/log";

export const dynamic = "force-dynamic";
export const maxDuration = 300;

/**
 * POST /api/agent/chat — SSE streaming chat with the agent.
 *
 * Uses OpenClaw's OpenAI-compatible /v1/chat/completions endpoint (streaming).
 * No WebSocket needed — pure HTTP fetch with streaming response.
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

  // Call OpenClaw's OpenAI-compatible chat completions API (streaming)
  const chatUrl = `${gatewayUrl}/v1/chat/completions`;

  let upstreamRes: Response;
  try {
    upstreamRes = await fetch(chatUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${gatewayToken}`,
      },
      body: JSON.stringify({
        model: "openclaw/default",
        messages: [{ role: "user", content }],
        stream: true,
      }),
    });
  } catch (err: any) {
    log("chat.gateway_error", { deploymentId, error: err.message });
    return NextResponse.json({ error: "Agent connection failed" }, { status: 502 });
  }

  if (!upstreamRes.ok || !upstreamRes.body) {
    const errText = await upstreamRes.text().catch(() => "");
    log("chat.upstream_error", { deploymentId, status: upstreamRes.status, error: errText });
    return NextResponse.json({ error: "Agent returned error" }, { status: 502 });
  }

  // Stream the upstream SSE response through to the browser
  const encoder = new TextEncoder();
  const reader = upstreamRes.body.getReader();
  const decoder = new TextDecoder();
  let fullText = "";

  const stream = new ReadableStream({
    async pull(controller) {
      try {
        const { done, value } = await reader.read();
        if (done) {
          // Save agent response to DB
          if (fullText) {
            await db.insert(agentMessages).values({
              deploymentId,
              direction: "agent_to_user",
              content: fullText,
            });
          }
          controller.enqueue(encoder.encode(`event: done\ndata: ${JSON.stringify({ fullText })}\n\n`));
          controller.close();
          return;
        }

        const chunk = decoder.decode(value, { stream: true });
        const lines = chunk.split("\n");

        for (const line of lines) {
          if (line.startsWith("data: ")) {
            const data = line.slice(6).trim();
            if (data === "[DONE]") continue;

            try {
              const parsed = JSON.parse(data);
              const token = parsed.choices?.[0]?.delta?.content;
              if (token) {
                fullText += token;
                controller.enqueue(encoder.encode(`event: token\ndata: ${JSON.stringify({ token })}\n\n`));
              }
            } catch { /* skip malformed chunks */ }
          }
        }
      } catch (err) {
        if (fullText) {
          await db.insert(agentMessages).values({
            deploymentId,
            direction: "agent_to_user",
            content: fullText,
          }).catch(() => {});
        }
        controller.enqueue(encoder.encode(`event: done\ndata: ${JSON.stringify({ fullText })}\n\n`));
        controller.close();
      }
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
