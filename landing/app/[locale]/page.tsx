import { useTranslations, useLocale } from "next-intl";
import Link from "next/link";
import LanguageSwitcher from "@/components/LanguageSwitcher";
import ConnectWalletButton from "@/components/ConnectWalletButton";
import LiveStats from "@/components/LiveStats";
import StrategyComparison from "@/components/StrategyComparison";

export default function Home() {
  const t = useTranslations("home");
  const locale = useLocale();

  return (
    <main className="min-h-screen">
      {/* Nav */}
      <nav className="fixed top-0 left-0 right-0 z-50 border-b border-border bg-bg/80 backdrop-blur-sm">
        <div className="mx-auto max-w-[1200px] px-6 flex items-center justify-between h-14">
          <span className="text-lg font-bold tracking-tight">
            a2ex<span className="text-accent">.</span>
          </span>
          <div className="flex items-center gap-4">
            <LanguageSwitcher />
            <ConnectWalletButton />
          </div>
        </div>
      </nav>

      {/* Hero */}
      <section className="relative pt-32 pb-20 scan-lines">
        <div className="mx-auto max-w-[1200px] px-6">
          <div className="inline-flex items-center gap-2 rounded-full bg-accent-subtle border border-accent/20 px-3.5 py-1.5 text-[13px] text-accent font-medium mb-6">
            <span className="w-1.5 h-1.5 rounded-full bg-success animate-pulse" />
            Live agents running
          </div>
          <h1 className="text-[clamp(40px,6vw,64px)] font-bold leading-[1.1] tracking-[-0.03em] mb-4">
            Your AI trades.<br />
            <span className="text-accent">You watch it grow.</span>
          </h1>
          <p className="text-lg text-text-muted max-w-[560px] mb-8">
            Deploy autonomous trading agents on decentralized compute. One wallet connection, one click, full control.
          </p>
          <div className="flex gap-3">
            <ConnectWalletButton />
            <Link
              href={`/${locale}/docs`}
              className="inline-flex items-center gap-2 rounded-md border border-border px-7 py-3.5 text-[15px] font-medium transition-all hover:border-accent hover:text-accent"
            >
              How it works
            </Link>
          </div>
          <LiveStats />
        </div>
      </section>

      {/* Strategy Comparison */}
      <section className="py-20">
        <div className="mx-auto max-w-[1200px] px-6">
          <p className="text-[13px] font-semibold text-accent uppercase tracking-[0.08em] mb-3">Strategies</p>
          <h2 className="text-[32px] font-bold tracking-[-0.02em] mb-4">Choose Your Agent</h2>
          <p className="text-text-muted text-base max-w-[560px] mb-12">
            Three strategies, transparent performance. Side-by-side, not a card grid.
          </p>
          <StrategyComparison />
        </div>
      </section>

      {/* How it works */}
      <section className="py-20 border-t border-border">
        <div className="mx-auto max-w-[1200px] px-6">
          <p className="text-[13px] font-semibold text-accent uppercase tracking-[0.08em] mb-3">How it works</p>
          <h2 className="text-[32px] font-bold tracking-[-0.02em] mb-12">Three Steps</h2>
          <div className="grid grid-cols-1 md:grid-cols-3 gap-8">
            {[
              { step: "01", title: "Connect", desc: "Link your Arbitrum wallet. We verify your identity with a signature — no passwords." },
              { step: "02", title: "Fund", desc: "Send USDC to your agent's hot wallet. Set your strategy, risk limits, and trade size." },
              { step: "03", title: "Deploy", desc: "One click deploys your agent to decentralized compute. Watch it trade in real time." },
            ].map((item) => (
              <div key={item.step} className="group">
                <div className="font-mono text-sm text-accent mb-3">{item.step}</div>
                <h3 className="text-xl font-semibold mb-2">{item.title}</h3>
                <p className="text-text-muted text-[15px] leading-relaxed">{item.desc}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Footer */}
      <footer className="py-12 border-t border-border">
        <div className="mx-auto max-w-[1200px] px-6 flex justify-between items-center text-[13px] text-text-muted">
          <span>a2ex — Autonomous AI Trading</span>
          <div className="flex gap-6">
            <Link href={`/${locale}/docs`} className="hover:text-accent transition">Docs</Link>
            <a href="https://github.com/IotrustGitHub/a2ex" target="_blank" rel="noopener noreferrer" className="hover:text-accent transition">GitHub</a>
          </div>
        </div>
      </footer>
    </main>
  );
}
