import type { ReactNode } from "react";
import type { Metadata } from "next";
import { NextIntlClientProvider } from "next-intl";
import { getMessages } from "next-intl/server";
import { notFound } from "next/navigation";
import { locales, type Locale } from "@/i18n/config";
import { Providers } from "@/app/providers";
import "../globals.css";

export const metadata: Metadata = {
  title: "a2ex — Autonomous AI Trading Agent",
  description:
    "Deploy autonomous trading agents on decentralized compute. One wallet, one click, full control.",
};

export default async function LocaleLayout({
  children,
  params,
}: {
  children: ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  if (!locales.includes(locale as Locale)) {
    notFound();
  }
  const messages = await getMessages();

  return (
    <html lang={locale} className="dark">
      <body className="antialiased" style={{ backgroundColor: "var(--bg)", color: "var(--text)" }}>
        <Providers>
          <NextIntlClientProvider messages={messages}>
            {children}
          </NextIntlClientProvider>
        </Providers>
      </body>
    </html>
  );
}
