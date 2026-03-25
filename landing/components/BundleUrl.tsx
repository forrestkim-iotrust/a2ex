"use client";

import { useState, useEffect } from "react";
import { useTranslations } from "next-intl";

export default function BundleUrl() {
  const [url, setUrl] = useState("");
  const [copied, setCopied] = useState(false);
  const t = useTranslations("quickstart");

  useEffect(() => {
    setUrl(`${window.location.origin}/bundle.json`);
  }, []);

  const handleCopy = async () => {
    if (!url) return;
    await navigator.clipboard.writeText(url);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="relative overflow-hidden rounded-xl border-2 border-primary/30 bg-gray-900">
      <div className="flex items-center gap-2 border-b border-gray-800 px-4 py-3">
        <div className="h-3 w-3 rounded-full bg-primary/60" />
        <span className="text-xs font-medium text-primary/80">
          {t("label")}
        </span>
      </div>
      <button
        onClick={handleCopy}
        className="absolute right-3 top-3 rounded bg-gray-700 px-2 py-1 text-xs text-gray-300 transition hover:bg-gray-600"
        aria-label="Copy to clipboard"
      >
        {copied ? t("copied") : "Copy"}
      </button>
      <div className="p-6">
        <p className="break-all font-mono text-lg font-medium leading-relaxed text-primary">
          {url || "loading..."}
        </p>
      </div>
    </div>
  );
}
