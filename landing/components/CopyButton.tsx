"use client";

import { useState } from "react";
import { useTranslations } from "next-intl";

export default function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const t = useTranslations("quickstart");

  const handleCopy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <button
      onClick={handleCopy}
      className="absolute right-3 top-3 rounded bg-gray-700 px-2 py-1 text-xs text-gray-300 transition hover:bg-gray-600"
      aria-label="Copy to clipboard"
    >
      {copied ? t("copied") : "Copy"}
    </button>
  );
}
