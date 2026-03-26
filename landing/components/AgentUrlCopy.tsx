"use client";

import { useState, useEffect } from "react";

export default function AgentUrlCopy() {
  const [url, setUrl] = useState("");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    setUrl(`${window.location.origin}/agent`);
  }, []);

  const handleCopy = async () => {
    if (!url) return;
    await navigator.clipboard.writeText(url);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div
      onClick={handleCopy}
      className="group flex cursor-pointer items-center justify-between rounded-lg border border-gray-700 bg-gray-900 px-5 py-4 transition hover:border-primary/50 hover:bg-gray-900/80"
    >
      <span className="font-mono text-sm text-gray-300 sm:text-base">
        {url || "..."}
      </span>
      <span className="ml-4 shrink-0 rounded bg-gray-800 px-3 py-1.5 text-xs font-medium text-gray-400 transition group-hover:bg-primary group-hover:text-white">
        {copied ? "Copied!" : "Copy"}
      </span>
    </div>
  );
}
