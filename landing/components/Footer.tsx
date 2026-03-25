import { useTranslations } from "next-intl";

export default function Footer() {
  const t = useTranslations("footer");

  return (
    <footer className="border-t border-gray-800 px-6 py-12">
      <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-6 sm:flex-row">
        <div>
          <span className="text-lg font-bold text-primary">a2ex</span>
          <p className="mt-1 text-sm text-gray-500">{t("tagline")}</p>
        </div>
        <div className="flex gap-6 text-sm text-gray-400">
          <a
            href="https://github.com/IotrustGitHub/a2ex"
            target="_blank"
            rel="noopener noreferrer"
            className="transition hover:text-gray-200"
          >
            {t("github")}
          </a>
          <a
            href="https://openclaw.ai"
            target="_blank"
            rel="noopener noreferrer"
            className="transition hover:text-gray-200"
          >
            {t("clawhub")}
          </a>
        </div>
      </div>
    </footer>
  );
}
