import { useTranslations, useLocale } from "next-intl";
import Link from "next/link";
import LanguageSwitcher from "@/components/LanguageSwitcher";
import AgentUrlCopy from "@/components/AgentUrlCopy";

export default function Home() {
  const t = useTranslations("home");
  const locale = useLocale();

  return (
    <main className="flex min-h-screen flex-col items-center justify-center px-6">
      {/* Language switcher — top right */}
      <div className="fixed right-6 top-6">
        <LanguageSwitcher />
      </div>

      <div className="w-full max-w-md">
        {/* Logo */}
        <h1 className="mb-12 text-2xl font-bold tracking-tight text-primary">
          a2ex
        </h1>

        {/* Headline */}
        <p className="mb-2 text-3xl font-bold leading-tight sm:text-4xl">
          {t("headline")}
        </p>
        <p className="mb-12 text-lg text-gray-500">{t("sub")}</p>

        {/* Instruction + Copy block */}
        <p className="mb-3 text-sm text-gray-400">{t("instruction")}</p>
        <AgentUrlCopy />

        {/* Links */}
        <div className="mt-12 flex gap-6 text-sm text-gray-500">
          <Link
            href={`/${locale}/docs`}
            className="transition hover:text-gray-300"
          >
            {t("docs")} &rarr;
          </Link>
          <a
            href="https://github.com/forrestkim-iotrust/a2ex"
            target="_blank"
            rel="noopener noreferrer"
            className="transition hover:text-gray-300"
          >
            {t("github")} &rarr;
          </a>
        </div>
      </div>
    </main>
  );
}
