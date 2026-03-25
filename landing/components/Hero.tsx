import { useTranslations, useLocale } from "next-intl";
import Link from "next/link";

export default function Hero() {
  const t = useTranslations("hero");
  const locale = useLocale();

  return (
    <section className="relative flex min-h-screen items-center justify-center overflow-hidden px-6 pt-20">
      <div className="pointer-events-none absolute inset-0 bg-gradient-to-b from-primary/5 via-transparent to-transparent" />

      <div className="relative mx-auto max-w-4xl text-center">
        <span className="mb-6 inline-block rounded-full border border-primary/30 bg-primary/10 px-4 py-1.5 text-sm font-medium text-primary">
          {t("badge")}
        </span>

        <h1 className="mb-6 text-5xl font-bold leading-tight tracking-tight sm:text-6xl lg:text-7xl">
          {t("title")}
        </h1>

        <p className="mx-auto mb-10 max-w-2xl text-lg text-gray-400 sm:text-xl">
          {t("subtitle")}
        </p>

        <div className="flex flex-col items-center justify-center gap-4 sm:flex-row">
          <a
            href="#quickstart"
            className="rounded-lg bg-primary px-8 py-3 font-semibold text-white transition hover:bg-primary/90"
          >
            {t("cta")}
          </a>
          <Link
            href={`/${locale}/docs`}
            className="rounded-lg border border-gray-700 px-8 py-3 font-semibold text-gray-300 transition hover:border-gray-500 hover:text-white"
          >
            {t("ctaDocs")}
          </Link>
        </div>

        {/* Simulated OpenClaw chat — shows the bundle URL flow */}
        <div className="mt-16 overflow-hidden rounded-xl border border-gray-800 bg-gray-900 shadow-2xl shadow-primary/5">
          <div className="flex items-center gap-2 border-b border-gray-800 px-4 py-3">
            <div className="h-3 w-3 rounded-full bg-red-500" />
            <div className="h-3 w-3 rounded-full bg-yellow-500" />
            <div className="h-3 w-3 rounded-full bg-green-500" />
            <span className="ml-2 text-xs text-gray-500">OpenClaw</span>
          </div>
          <div className="p-6 text-left text-sm leading-relaxed">
            {/* User pastes bundle URL */}
            <div className="mb-4 flex justify-end">
              <div className="max-w-md rounded-lg bg-primary/20 px-4 py-2.5 font-mono text-xs text-primary break-all">
                https://a2ex-landing.vercel.app/bundle.json
              </div>
            </div>

            {/* AI responses — the 3-turn flow */}
            <div className="space-y-3">
              <p className="text-gray-400">
                <span className="font-semibold text-primary">AI:</span>{" "}
                Reading bundle... a2ex plugin required.
              </p>
              <p className="text-gray-500 font-mono text-xs">
                exec: openclaw plugin install openclaw-plugin-a2ex
              </p>
              <p className="text-gray-400">
                <span className="font-semibold text-primary">AI:</span>{" "}
                Plugin installed.{" "}
                <span className="text-green-400">&#10003;</span>{" "}
                Bootstrapping wallets...
              </p>
              <p className="text-gray-400">
                <span className="font-semibold text-primary">AI:</span>{" "}
                Vault:{" "}
                <span className="font-mono text-gray-300">0x7a3F...c29E</span>
                {" "}| Hot: ready ($50 limit){" "}
                <span className="text-green-400">&#10003;</span>
              </p>
              <p className="text-gray-400">
                <span className="font-semibold text-primary">AI:</span>{" "}
                Send <span className="text-white">15 USDC + 0.005 ETH</span> to vault on Arbitrum.
              </p>
              <p className="text-gray-400">
                <span className="font-semibold text-primary">AI:</span>{" "}
                Funded. Bridging Arbitrum → Polygon...
              </p>
              <p className="text-gray-400">
                <span className="font-semibold text-primary">AI:</span>{" "}
                <span className="text-green-400">Trade executed:</span>{" "}
                Polymarket — YES @ $0.62 — 5 USDC
              </p>
              <p className="text-gray-400">
                <span className="font-semibold text-primary">AI:</span>{" "}
                <span className="text-green-400">Trade executed:</span>{" "}
                Hyperliquid — ETH-PERP Long @ $3,841
              </p>
            </div>
            <p className="mt-4 animate-pulse text-gray-600">▋</p>
          </div>
        </div>
      </div>
    </section>
  );
}
