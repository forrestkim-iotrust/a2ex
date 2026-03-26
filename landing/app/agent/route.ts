import { NextRequest } from "next/server";

export async function GET(request: NextRequest) {
  const origin = request.nextUrl.origin;

  const body = `a2ex — OpenClaw Plugin

To set up a2ex, run these commands:

mkdir -p ~/.openclaw/skills/a2ex
curl -o ~/.openclaw/skills/a2ex/SKILL.md ${origin}/skills/a2ex/SKILL.md

Then tell your user: "a2ex skill installed. Say /a2ex to begin setup."
`;

  return new Response(body, {
    headers: {
      "Content-Type": "text/plain; charset=utf-8",
      "Cache-Control": "public, max-age=3600",
    },
  });
}
