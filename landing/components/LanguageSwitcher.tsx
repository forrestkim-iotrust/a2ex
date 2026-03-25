"use client";

import { useLocale, useTranslations } from "next-intl";
import { useRouter, usePathname } from "next/navigation";
import { locales, type Locale } from "@/i18n/config";

export default function LanguageSwitcher() {
  const locale = useLocale();
  const router = useRouter();
  const pathname = usePathname();
  const t = useTranslations("lang");

  const switchLocale = (newLocale: Locale) => {
    const segments = pathname.split("/");
    segments[1] = newLocale;
    router.push(segments.join("/"));
  };

  return (
    <div className="flex items-center gap-1 text-sm">
      {locales.map((l) => (
        <button
          key={l}
          onClick={() => switchLocale(l)}
          className={`rounded px-2 py-1 transition ${
            locale === l
              ? "bg-primary/20 text-primary"
              : "text-gray-400 hover:text-gray-200"
          }`}
        >
          {t(l)}
        </button>
      ))}
    </div>
  );
}
