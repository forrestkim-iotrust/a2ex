import { useTranslations, useLocale } from "next-intl";
import Link from "next/link";
import LanguageSwitcher from "@/components/LanguageSwitcher";

export default function DocsPage() {
  const t = useTranslations("docs");
  const locale = useLocale();
  const safetyItems = t.raw("safety.items") as string[];

  return (
    <main className="min-h-screen px-6 py-16">
      <div className="fixed right-6 top-6">
        <LanguageSwitcher />
      </div>

      <div className="mx-auto max-w-lg">
        <Link
          href={`/${locale}`}
          className="mb-8 inline-block text-sm text-gray-500 transition hover:text-gray-300"
        >
          &larr; {t("back")}
        </Link>

        <h1 className="mb-2 text-2xl font-bold">{t("title")}</h1>
        <p className="mb-12 text-gray-500">{t("subtitle")}</p>

        <div className="space-y-10">
          {(["step1", "step2", "step3", "step4"] as const).map((key) => (
            <div key={key}>
              <h2 className="mb-2 text-lg font-semibold">
                {t(`${key}.title`)}
              </h2>
              <p className="text-gray-400">{t(`${key}.description`)}</p>
            </div>
          ))}
        </div>

        <div className="mt-16 rounded-lg border border-gray-800 p-6">
          <h2 className="mb-4 text-lg font-semibold">{t("safety.title")}</h2>
          <ul className="space-y-2 text-sm text-gray-400">
            {safetyItems.map((item: string, i: number) => (
              <li key={i}>{item}</li>
            ))}
          </ul>
        </div>
      </div>
    </main>
  );
}
