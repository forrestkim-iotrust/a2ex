# Design System — a2ex

## Product Context
- **What this is:** AI autonomous crypto trading agent platform — deploy, monitor, and control trading agents from the browser
- **Who it's for:** Crypto traders, DeFi users, agent operators seeking automated trading with full transparency
- **Space/industry:** AI agent crypto trading (peers: NickAI, Olas/Polystrat, Virtuals Protocol)
- **Project type:** Hybrid — marketing landing page + app-like dashboard/control center

## Aesthetic Direction
- **Direction:** Retro-Futuristic Terminal — Bloomberg terminal's gravitas meets modern agent platform innovation
- **Decoration level:** Intentional — subtle grid overlay, scan-line texture on hero, no decorative blobs
- **Mood:** Professional, trustworthy, alive. The product should feel like a control room for autonomous agents — calm but alert, data-dense but readable. Amber warmth cuts through the cold dark terminal aesthetic.
- **Key differentiation:** Amber/gold accent when every competitor uses teal/cyan or purple. Side-by-side strategy comparison instead of card grids. Sidebar+main dashboard layout instead of stacked cards.

## Typography
- **Display/Hero:** DM Sans 700 — geometric, confident, readable at large sizes
- **Body:** DM Sans 400/500 — pairs naturally with display, excellent readability
- **UI/Labels:** DM Sans 500/600
- **Data/Tables:** JetBrains Mono 500 — tabular-nums, terminal aesthetic, precise alignment
- **Code:** JetBrains Mono 400
- **Loading:** Google Fonts CDN — `family=DM+Sans:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600`
- **Scale:**
  - `xs`: 12px — captions, timestamps
  - `sm`: 13px — labels, helper text
  - `base`: 14px — UI text, buttons
  - `md`: 16px — body text
  - `lg`: 18px — subtitles
  - `xl`: 20px — section headers
  - `2xl`: 24px — page titles
  - `3xl`: 32px — section heroes
  - `4xl`: 48px — landing hero
  - `5xl`: 64px — hero display (clamp)

## Color

### Approach: Restrained — amber accent + warm neutrals. Green/red reserved for P&L only.

### Dark Mode (Primary)
```css
--bg: #0c0c0e;              /* warm black */
--surface: #161618;          /* elevated surface */
--surface-elevated: #1e1e20; /* cards, modals */
--text: #e8e4de;             /* warm off-white */
--text-muted: #6b6560;       /* warm gray */
--accent: #f0a030;           /* amber/gold — primary action, brand */
--accent-hover: #f5b547;     /* amber lighter */
--accent-subtle: rgba(240, 160, 48, 0.1); /* amber bg tint */
--success: #22c55e;          /* profit, active, confirmed */
--danger: #ef4444;           /* loss, error, kill switch */
--warning: #f59e0b;          /* caution, pending */
--info: #3b82f6;             /* informational */
--border: #2a2a2c;           /* dividers, card borders */
```

### Light Mode (Secondary)
```css
--bg: #f5f3ef;
--surface: #ffffff;
--surface-elevated: #fafaf8;
--text: #1a1815;
--text-muted: #8a857f;
--accent: #d48a20;
--accent-hover: #c07a18;
--accent-subtle: rgba(212, 138, 32, 0.08);
--success: #16a34a;
--danger: #dc2626;
--warning: #d97706;
--info: #2563eb;
--border: #e0ddd8;
```

### Rules
- Green and red are ONLY for P&L / profit-loss indicators. Never decorative.
- Amber accent is for: primary CTAs, active states, brand elements, section labels.
- Avoid purple, teal, or cyan — these are competitor territory.

## Spacing
- **Base unit:** 4px
- **Density:** Comfortable (landing) / Compact (dashboard)
- **Scale:** 2xs(2) xs(4) sm(8) md(16) lg(24) xl(32) 2xl(48) 3xl(64) 4xl(96)
- **Content padding:** 24px (mobile), 32px (tablet), 48px (desktop)
- **Section gaps:** 80px (landing), 16px (dashboard panels)

## Layout
- **Approach:** Hybrid — editorial for landing, terminal-dense for dashboard
- **Grid:** 12 columns, 24px gutter
- **Max content width:** 1200px
- **Border radius:**
  - `sm`: 4px — buttons, inputs, badges
  - `md`: 8px — cards, panels
  - `lg`: 12px — modals, containers
  - `full`: 9999px — pills, status dots
- **Dashboard:** Sidebar (240px fixed) + Main (fluid) — sidebar collapses to bottom sheet on mobile
- **Landing:** Single column, full-width sections, contained content

## Motion
- **Approach:** Intentional — motion supports hierarchy and trust, never decorative
- **Easing:** enter(ease-out) exit(ease-in) move(ease-in-out)
- **Duration:**
  - `micro`: 50-100ms — hover states, toggles
  - `short`: 150-250ms — button feedback, input focus
  - `medium`: 250-400ms — panel transitions, card reveals
  - `long`: 400-700ms — page transitions, hero animations
- **Specific animations:**
  - Hero stats: count-up on load (800ms, ease-out)
  - Strategy sparklines: bars grow on scroll-into-view (400ms staggered)
  - Deploy progress: smooth step transitions with labeled stages
  - First trade confetti: canvas-confetti, 2s burst
  - Kill Switch: button turns red with scale pulse on confirm
- **Reduced motion:** Respect `prefers-reduced-motion` — disable all animations, show static states

## Responsive
- **Mobile:** <768px — single column, sidebar → bottom tabs, touch targets 44px min
- **Tablet:** 768-1024px — sidebar narrows to 200px, main stretches
- **Desktop:** >1024px — full sidebar (240px) + main
- **Dashboard mobile:** Status/P&L/Chat as swipeable horizontal tabs, Kill Switch sticky at bottom

## Accessibility
- **Color contrast:** WCAG AA minimum (4.5:1 text, 3:1 UI components)
- **Focus indicators:** 2px amber outline with 2px offset
- **Keyboard navigation:** Full tab support through all interactive elements
- **ARIA landmarks:** main, nav, aside, region for each dashboard panel
- **Screen readers:** Live regions for P&L updates, trade notifications, agent status changes
- **Touch targets:** 44px minimum on mobile

## Anti-Patterns (Never Do)
- Purple/violet gradients or teal/cyan accents (competitor territory)
- 3-column feature grids with icons in colored circles
- Centered-everything layout
- Uniform bubbly border-radius on all elements
- Decorative blobs, floating circles, wavy SVG dividers
- Generic hero copy ("Welcome to a2ex", "Unlock the power of...")
- Card grids for <5 items (use comparison layout)
- Stacked card dashboard layout (use sidebar + main)

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-03-26 | Initial design system created | Created by /design-consultation based on competitive research (NickAI, Olas, Virtuals, ElizaOS, Hyperliquid, Drift). Amber accent chosen for differentiation from teal/purple saturation. |
| 2026-03-26 | DM Sans over Inter/General Sans | DM Sans pairs geometric confidence with body readability. Inter is overused. General Sans considered but DM Sans has better body text weights. |
| 2026-03-26 | Side-by-side strategies over card grid | With 3 strategies at launch, a card grid reads as a demo. Comparison layout is more honest and effective. |
| 2026-03-26 | Sidebar + main dashboard | Stacked cards is an AI slop anti-pattern. Sidebar separates control (status, kill switch) from data (P&L, trades, chat). |
