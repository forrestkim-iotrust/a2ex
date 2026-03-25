import { useTranslations } from "next-intl";

export default function Architecture() {
  const t = useTranslations("architecture");

  return (
    <section className="px-6 py-24">
      <div className="mx-auto max-w-5xl">
        <h2 className="mb-4 text-center text-3xl font-bold sm:text-4xl">
          {t("title")}
        </h2>
        <p className="mb-16 text-center text-gray-400">{t("description")}</p>

        {/* ASCII-style architecture diagram */}
        <div className="overflow-x-auto rounded-xl border border-gray-800 bg-gray-900/50 p-8">
          <div className="min-w-[600px]">
            {/* Docker container boundary */}
            <div className="rounded-lg border border-dashed border-gray-600 p-6">
              <p className="mb-4 text-xs font-medium uppercase tracking-wider text-gray-500">
                OpenClaw Docker Container
              </p>

              <div className="grid grid-cols-4 gap-4">
                {/* OpenClaw */}
                <div className="rounded-lg bg-primary/20 p-4 text-center">
                  <div className="mb-1 text-sm font-bold text-primary">
                    {t("openclaw")}
                  </div>
                  <div className="text-xs text-gray-400">
                    {t("openclawDesc")}
                  </div>
                </div>

                {/* Plugin */}
                <div className="rounded-lg bg-accent/20 p-4 text-center">
                  <div className="mb-1 text-sm font-bold text-accent">
                    {t("plugin")}
                  </div>
                  <div className="text-xs text-gray-400">
                    {t("pluginDesc")}
                  </div>
                </div>

                {/* a2ex */}
                <div className="rounded-lg bg-rust/20 p-4 text-center">
                  <div className="mb-1 text-sm font-bold text-rust">
                    {t("a2ex")}
                  </div>
                  <div className="text-xs text-gray-400">{t("a2exDesc")}</div>
                </div>

                {/* WAIaaS */}
                <div className="rounded-lg bg-vault/20 p-4 text-center">
                  <div className="mb-1 text-sm font-bold text-vault">
                    {t("waiaas")}
                  </div>
                  <div className="text-xs text-gray-400">
                    {t("waiaasDesc")}
                  </div>
                </div>
              </div>

              {/* Connection arrows */}
              <div className="mt-4 flex items-center justify-center gap-2 text-xs text-gray-500">
                <span>MCP stdio</span>
                <span>|</span>
                <span>HTTP</span>
                <span>|</span>
                <span>Tool calls</span>
              </div>
            </div>

            {/* Arrow down */}
            <div className="my-4 flex justify-center">
              <svg
                className="h-8 w-8 text-gray-600"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={1.5}
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M19.5 13.5L12 21m0 0l-7.5-7.5M12 21V3"
                />
              </svg>
            </div>

            {/* External venues */}
            <div className="grid grid-cols-3 gap-4">
              <div className="rounded-lg border border-gray-700 bg-gray-800/50 p-4 text-center">
                <div className="mb-1 text-sm font-semibold text-gray-200">
                  {t("polymarket")}
                </div>
                <div className="text-xs text-gray-500">Prediction Markets</div>
              </div>
              <div className="rounded-lg border border-gray-700 bg-gray-800/50 p-4 text-center">
                <div className="mb-1 text-sm font-semibold text-gray-200">
                  {t("hyperliquid")}
                </div>
                <div className="text-xs text-gray-500">Perps DEX</div>
              </div>
              <div className="rounded-lg border border-gray-700 bg-gray-800/50 p-4 text-center">
                <div className="mb-1 text-sm font-semibold text-gray-200">
                  {t("across")}
                </div>
                <div className="text-xs text-gray-500">Cross-Chain</div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
