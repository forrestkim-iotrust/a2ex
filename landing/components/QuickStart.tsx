import { useTranslations } from "next-intl";
import BundleUrl from "./BundleUrl";

export default function QuickStart() {
  const t = useTranslations("quickstart");

  return (
    <section id="quickstart" className="px-6 py-24">
      <div className="mx-auto max-w-3xl">
        <h2 className="mb-4 text-center text-3xl font-bold sm:text-4xl">
          {t("title")}
        </h2>
        <p className="mb-12 text-center text-gray-400">{t("description")}</p>

        <BundleUrl />

        <p className="mt-8 text-center text-sm text-gray-500">{t("result")}</p>
      </div>
    </section>
  );
}
