import { useTranslations } from "next-intl";

const features = [
  {
    key: "trading",
    icon: (
      <svg className="h-8 w-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M3.75 3v11.25A2.25 2.25 0 006 16.5h2.25M3.75 3h-1.5m1.5 0h16.5m0 0h1.5m-1.5 0v11.25A2.25 2.25 0 0118 16.5h-2.25m-7.5 0h7.5m-7.5 0l-1 3m8.5-3l1 3m0 0l.5 1.5m-.5-1.5h-9.5m0 0l-.5 1.5M9 11.25v1.5M12 9v3.75m3-6v6" />
      </svg>
    ),
    color: "text-primary",
  },
  {
    key: "security",
    icon: (
      <svg className="h-8 w-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75L11.25 15 15 9.75m-3-7.036A11.959 11.959 0 013.598 6 11.99 11.99 0 003 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285z" />
      </svg>
    ),
    color: "text-vault",
  },
  {
    key: "multichain",
    icon: (
      <svg className="h-8 w-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M7.5 21L3 16.5m0 0L7.5 12M3 16.5h13.5m0-13.5L21 7.5m0 0L16.5 12M21 7.5H7.5" />
      </svg>
    ),
    color: "text-hot",
  },
  {
    key: "simple",
    icon: (
      <svg className="h-8 w-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M21 7.5l-2.25-1.313M21 7.5v2.25m0-2.25l-2.25 1.313M3 7.5l2.25-1.313M3 7.5l2.25 1.313M3 7.5v2.25m9 3l2.25-1.313M12 12.75l-2.25-1.313M12 12.75V15m0 6.75l2.25-1.313M12 21.75V15m0 0l-2.25-1.313M3 16.5v2.25m0 0l2.25 1.313M3 18.75l2.25-1.313m13.5 0l2.25 1.313M21 18.75l-2.25-1.313M21 18.75v-2.25m0 0l-2.25-1.313" />
      </svg>
    ),
    color: "text-accent",
  },
] as const;

export default function Features() {
  const t = useTranslations("features");

  return (
    <section className="px-6 py-24">
      <div className="mx-auto max-w-6xl">
        <h2 className="mb-16 text-center text-3xl font-bold sm:text-4xl">
          {t("title")}
        </h2>
        <div className="grid gap-8 sm:grid-cols-2 lg:grid-cols-4">
          {features.map(({ key, icon, color }) => (
            <div
              key={key}
              className="rounded-xl border border-gray-800 bg-gray-900/50 p-6 transition hover:border-gray-700"
            >
              <div className={`mb-4 ${color}`}>{icon}</div>
              <h3 className="mb-2 text-lg font-semibold">
                {t(`${key}.title`)}
              </h3>
              <p className="text-sm leading-relaxed text-gray-400">
                {t(`${key}.description`)}
              </p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
