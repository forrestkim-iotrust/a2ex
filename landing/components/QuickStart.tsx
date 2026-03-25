import { useTranslations } from "next-intl";
import CopyButton from "./CopyButton";

export default function QuickStart() {
  const t = useTranslations("quickstart");

  return (
    <section id="quickstart" className="px-6 py-24">
      <div className="mx-auto max-w-3xl">
        <h2 className="mb-4 text-center text-3xl font-bold sm:text-4xl">
          {t("title")}
        </h2>
        <p className="mb-12 text-center text-gray-400">{t("description")}</p>

        {/* The bundle URL */}
        <div className="relative overflow-hidden rounded-xl border-2 border-primary/30 bg-gray-900">
          <div className="flex items-center gap-2 border-b border-gray-800 px-4 py-3">
            <div className="h-3 w-3 rounded-full bg-primary/60" />
            <span className="text-xs font-medium text-primary/80">
              {t("label")}
            </span>
          </div>
          <CopyButton text={t("bundleUrl")} />
          <div className="p-6">
            <p className="break-all font-mono text-lg font-medium leading-relaxed text-primary">
              {t("bundleUrl")}
            </p>
          </div>
        </div>

        <p className="mt-8 text-center text-sm text-gray-500">{t("result")}</p>
      </div>
    </section>
  );
}
