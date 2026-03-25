import { useTranslations, useLocale } from "next-intl";
import Link from "next/link";
import LanguageSwitcher from "./LanguageSwitcher";

export default function Nav() {
  const t = useTranslations("nav");
  const locale = useLocale();

  return (
    <nav className="fixed left-0 right-0 top-0 z-50 border-b border-gray-800 bg-gray-950/80 backdrop-blur-md">
      <div className="mx-auto flex max-w-6xl items-center justify-between px-6 py-4">
        <Link href={`/${locale}`} className="text-xl font-bold">
          <span className="text-primary">a2ex</span>
        </Link>
        <div className="flex items-center gap-6">
          <Link
            href={`/${locale}/docs`}
            className="text-sm text-gray-400 transition hover:text-gray-200"
          >
            {t("docs")}
          </Link>
          <a
            href="https://github.com/IotrustGitHub/a2ex"
            target="_blank"
            rel="noopener noreferrer"
            className="text-sm text-gray-400 transition hover:text-gray-200"
          >
            {t("github")}
          </a>
          <a
            href="https://openclaw.ai"
            target="_blank"
            rel="noopener noreferrer"
            className="text-sm text-gray-400 transition hover:text-gray-200"
          >
            {t("clawhub")}
          </a>
          <LanguageSwitcher />
        </div>
      </div>
    </nav>
  );
}
