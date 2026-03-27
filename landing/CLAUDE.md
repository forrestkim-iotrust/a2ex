# a2ex Landing

Phase 2 — 지갑 연결 + Akash 원클릭 배포 + 라이브 대시보드.

## Design System
Always read DESIGN.md before making any visual or UI decisions.
All font choices, colors, spacing, and aesthetic direction are defined there.
Do not deviate without explicit user approval.
In QA mode, flag any code that doesn't match DESIGN.md.

## Tech Stack
- Next.js 14 (App Router)
- Tailwind CSS
- wagmi v2 + RainbowKit (wallet connection)
- Drizzle ORM + Neon Serverless (neon-http driver)
- Upstash Redis (command channel)
- Framer Motion (animations)
- Vitest (testing)
- iron-session + SIWE (auth)

## Key Patterns
- `ssr: true` in wagmi config for hydration
- `'use client'` Providers component in `app/providers.tsx`
- Akash Console Managed Wallet API (AEP-63) for deploys
- All API routes use SIWE session middleware
