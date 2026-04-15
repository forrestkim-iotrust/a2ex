"use client";

import { useState, useEffect, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { QRCodeSVG } from "qrcode.react";

interface FundingModalProps {
  hotAddress: string;
  strategyId: string;
  minFundUsd: number;
  currentBalance?: string;
  onClose: () => void;
  onFunded?: () => void;
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <button
      onClick={handleCopy}
      className="px-3 py-1.5 text-xs font-semibold rounded-sm transition-all min-h-[32px] border border-border hover:border-accent hover:text-accent"
    >
      {copied ? "Copied!" : "Copy"}
    </button>
  );
}

export default function FundingModal({
  hotAddress,
  strategyId,
  minFundUsd,
  currentBalance,
  onClose,
  onFunded,
}: FundingModalProps) {
  const [pollBalance, setPollBalance] = useState(currentBalance);
  const balance = parseFloat(pollBalance || "0");
  const funded = balance >= minFundUsd;

  // Poll for balance updates
  useEffect(() => {
    if (funded) return;
    // Parent component handles polling — we just react to prop changes
  }, [funded]);

  useEffect(() => {
    setPollBalance(currentBalance);
  }, [currentBalance]);

  useEffect(() => {
    if (funded && onFunded) {
      const timer = setTimeout(onFunded, 1500);
      return () => clearTimeout(timer);
    }
  }, [funded, onFunded]);

  // Close on Escape
  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onClose]);

  const networkLabel = strategyId.includes("hyperliquid") ? "Arbitrum" : "Polygon";
  const tokenLabel = strategyId.includes("hyperliquid") ? "USDC" : "USDC.e";

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        className="fixed inset-0 z-50 flex items-center justify-center bg-bg/80 backdrop-blur-sm p-4"
        onClick={(e) => e.target === e.currentTarget && onClose()}
      >
        <motion.div
          initial={{ opacity: 0, scale: 0.95, y: 8 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: 8 }}
          className="bg-surface border border-border rounded-lg w-full max-w-[420px] overflow-hidden"
          role="dialog"
          aria-modal="true"
          aria-label="Fund your agent"
        >
          {/* Header */}
          <div className="flex items-center justify-between px-6 py-4 border-b border-border">
            <h2 className="text-lg font-semibold">Fund Your Agent</h2>
            <button
              onClick={onClose}
              className="w-8 h-8 flex items-center justify-center text-text-muted hover:text-text transition-colors rounded-sm"
              aria-label="Close"
            >
              &times;
            </button>
          </div>

          {/* Body */}
          <div className="px-6 py-6 space-y-6">
            {/* Network notice */}
            <div className="bg-accent-subtle border border-accent/15 rounded-sm px-4 py-3">
              <div className="text-xs font-semibold text-accent mb-1">
                Send {tokenLabel} on {networkLabel}
              </div>
              <div className="text-xs text-text-muted">
                Minimum ${minFundUsd} required. Only send {tokenLabel} on the {networkLabel} network.
              </div>
            </div>

            {/* QR Code */}
            <div className="flex justify-center">
              <div className="bg-white p-4 rounded-md">
                <QRCodeSVG
                  value={hotAddress}
                  size={180}
                  level="M"
                  bgColor="#ffffff"
                  fgColor="#0c0c0e"
                />
              </div>
            </div>

            {/* Address */}
            <div>
              <div className="text-xs text-text-muted mb-2">Deposit Address</div>
              <div className="flex items-center gap-2 bg-bg border border-border rounded-sm px-3 py-2.5">
                <span className="font-mono text-xs flex-1 break-all select-all">
                  {hotAddress}
                </span>
                <CopyButton text={hotAddress} />
              </div>
            </div>

            {/* Balance tracker */}
            <div className="bg-bg border border-border rounded-sm px-4 py-3">
              <div className="flex items-center justify-between mb-2">
                <span className="text-xs text-text-muted">Current Balance</span>
                <span className="text-xs text-text-muted">Min: ${minFundUsd}</span>
              </div>
              <div className="flex items-center justify-between">
                <span className={`font-mono text-lg font-semibold ${funded ? "text-success" : "text-accent"}`}>
                  ${balance.toFixed(2)}
                </span>
                {funded ? (
                  <motion.span
                    initial={{ scale: 0 }}
                    animate={{ scale: 1 }}
                    className="text-xs font-semibold text-success bg-success/10 px-2.5 py-1 rounded-full"
                  >
                    Funded
                  </motion.span>
                ) : (
                  <span className="text-xs text-text-muted flex items-center gap-1.5">
                    <span className="w-1.5 h-1.5 rounded-full bg-accent animate-pulse" />
                    Waiting for deposit...
                  </span>
                )}
              </div>
              {/* Progress bar */}
              <div className="mt-3 h-1.5 bg-border rounded-full overflow-hidden">
                <motion.div
                  className={`h-full rounded-full ${funded ? "bg-success" : "bg-accent"}`}
                  initial={{ width: 0 }}
                  animate={{ width: `${Math.min((balance / minFundUsd) * 100, 100)}%` }}
                  transition={{ duration: 0.5, ease: "easeOut" }}
                />
              </div>
            </div>
          </div>

          {/* Footer */}
          <div className="px-6 py-4 border-t border-border">
            <button
              onClick={onClose}
              className={`w-full py-3 text-center font-semibold text-sm rounded-sm transition-all min-h-[44px] ${
                funded
                  ? "bg-accent text-bg hover:bg-accent-hover"
                  : "bg-border/50 text-text-muted hover:bg-border"
              }`}
            >
              {funded ? "Continue to Dashboard" : "I'll fund later"}
            </button>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  );
}
