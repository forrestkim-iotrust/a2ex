import { useTranslations, useLocale } from "next-intl";
import Link from "next/link";
import Nav from "@/components/Nav";
import Footer from "@/components/Footer";
import CopyButton from "@/components/CopyButton";
import BundleUrl from "@/components/BundleUrl";

function PromptCard({
  number,
  title,
  description,
  prompt,
  note,
  warning,
}: {
  number: number;
  title: string;
  description: string;
  prompt: string;
  note: string;
  warning?: string;
}) {
  return (
    <div className="rounded-xl border border-gray-800 bg-gray-900/50 p-6">
      <div className="mb-4 flex items-center gap-3">
        <span className="flex h-8 w-8 items-center justify-center rounded-full bg-primary/20 text-sm font-bold text-primary">
          {number}
        </span>
        <h3 className="text-lg font-semibold">{title}</h3>
      </div>
      <p className="mb-4 text-gray-400">{description}</p>

      {/* What to say in OpenClaw */}
      <div className="relative mb-4 overflow-hidden rounded-lg border border-primary/20 bg-primary/5">
        <CopyButton text={prompt} />
        <div className="p-4">
          <p className="text-xs font-medium uppercase tracking-wider text-primary/60 mb-2">
            Say in OpenClaw:
          </p>
          <p className="font-medium text-gray-200">
            &ldquo;{prompt}&rdquo;
          </p>
        </div>
      </div>

      <p className="text-sm text-gray-500">{note}</p>

      {warning && (
        <div className="mt-3 rounded-lg border border-red-500/20 bg-red-500/5 p-3 text-sm text-red-400">
          {warning}
        </div>
      )}
    </div>
  );
}

export default function DocsPage() {
  const t = useTranslations("docs");
  const locale = useLocale();
  const limitations = t.raw("limitations.items") as string[];

  return (
    <>
      <Nav />
      <main className="px-6 pb-24 pt-32">
        <div className="mx-auto max-w-3xl">
          {/* Header */}
          <div className="mb-12">
            <Link
              href={`/${locale}`}
              className="mb-6 inline-block text-sm text-gray-500 transition hover:text-gray-300"
            >
              &larr; {t("backToHome")}
            </Link>
            <h1 className="mb-4 text-4xl font-bold">{t("title")}</h1>
            <p className="text-lg text-gray-400">{t("subtitle")}</p>
          </div>

          {/* Prerequisites */}
          <div className="mb-8 rounded-xl border border-gray-800 bg-gray-900/50 p-6">
            <h2 className="mb-4 text-lg font-semibold">
              {t("prerequisites.title")}
            </h2>
            <ul className="space-y-2 text-gray-400">
              <li className="flex items-start gap-2">
                <span className="mt-1 text-green-400">&#10003;</span>
                {t("prerequisites.openclaw")}
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1 text-green-400">&#10003;</span>
                {t("prerequisites.funds")}
              </li>
            </ul>
            <p className="mt-4 text-sm text-gray-500">
              {t("prerequisites.note")}
            </p>
          </div>

          {/* Quick way — the one-liner */}
          <div className="mb-12 rounded-xl border-2 border-primary/30 bg-primary/5 p-6">
            <h2 className="mb-2 text-xl font-bold text-primary">
              {t("quick.title")}
            </h2>
            <p className="mb-4 text-gray-400">{t("quick.description")}</p>
            <BundleUrl />
            <p className="mt-4 text-sm text-gray-500">{t("quick.note")}</p>
          </div>

          {/* Divider */}
          <div className="mb-12 flex items-center gap-4">
            <div className="h-px flex-1 bg-gray-800" />
            <span className="text-sm text-gray-600">or step by step</span>
            <div className="h-px flex-1 bg-gray-800" />
          </div>

          {/* Steps */}
          <div className="space-y-6">
            <PromptCard
              number={1}
              title={t("step1.title")}
              description={t("step1.description")}
              prompt={t("step1.prompt")}
              note={t("step1.note")}
            />
            <PromptCard
              number={2}
              title={t("step2.title")}
              description={t("step2.description")}
              prompt={t("step2.prompt")}
              note={t("step2.note")}
              warning={t("step2.warning")}
            />
            <PromptCard
              number={3}
              title={t("step3.title")}
              description={t("step3.description")}
              prompt={t("step3.prompt")}
              note={t("step3.note")}
            />
            <PromptCard
              number={4}
              title={t("step4.title")}
              description={t("step4.description")}
              prompt={t("step4.prompt")}
              note={t("step4.note")}
            />
          </div>

          {/* Known Limitations */}
          <div className="mt-8 rounded-xl border border-accent/20 bg-accent/5 p-6">
            <h2 className="mb-4 text-lg font-semibold text-accent">
              {t("limitations.title")}
            </h2>
            <ul className="space-y-2 text-sm text-gray-400">
              {limitations.map((item: string, i: number) => (
                <li key={i} className="flex items-start gap-2">
                  <span className="mt-0.5 text-accent">&#9888;</span>
                  {item}
                </li>
              ))}
            </ul>
          </div>
        </div>
      </main>
      <Footer />
    </>
  );
}
