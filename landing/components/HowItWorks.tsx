import { useTranslations } from "next-intl";

export default function HowItWorks() {
  const t = useTranslations("howItWorks");
  const steps = t.raw("steps") as Array<{
    num: string;
    title: string;
    description: string;
  }>;

  return (
    <section className="px-6 py-24">
      <div className="mx-auto max-w-4xl">
        <h2 className="mb-16 text-center text-3xl font-bold sm:text-4xl">
          {t("title")}
        </h2>

        <div className="relative">
          {/* Vertical line */}
          <div className="absolute left-6 top-0 h-full w-px bg-gradient-to-b from-primary/50 via-primary/20 to-transparent sm:left-8" />

          <div className="space-y-12">
            {steps.map((step, i) => (
              <div key={i} className="relative flex gap-6 sm:gap-8">
                <div className="relative z-10 flex h-12 w-12 shrink-0 items-center justify-center rounded-full border border-primary/30 bg-gray-950 text-lg font-bold text-primary sm:h-16 sm:w-16">
                  {step.num}
                </div>
                <div className="pt-2 sm:pt-4">
                  <h3 className="mb-2 text-lg font-semibold">{step.title}</h3>
                  <p className="text-gray-400">{step.description}</p>
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
