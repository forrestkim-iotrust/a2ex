import type { ReactNode } from "react";
import type { Metadata } from "next";
import { NextIntlClientProvider } from "next-intl";
import { getMessages } from "next-intl/server";
import { notFound } from "next/navigation";
import { locales, type Locale } from "@/i18n/config";
import "../globals.css";

export const metadata: Metadata = {
  title: "a2ex — Autonomous AI Trading Agent",
  description:
    "AI agents that trade autonomously across prediction markets and DeFi venues. Non-custodial. Multi-chain. Powered by OpenClaw.",
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
      <body className="bg-gray-950 text-gray-100 antialiased">
        <NextIntlClientProvider messages={messages}>
          {children}
        </NextIntlClientProvider>
      </body>
    </html>
  );
}
