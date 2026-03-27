"use client";

import { useEffect, useState, useRef, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { useSearchParams } from "next/navigation";
import { useFirstTradeConfetti } from "@/lib/hooks/useFirstTradeConfetti";

interface Trade {
  id: string;
  venue: string;
  action: string;
  amountUsd: string;
  pnlUsd: string;
  ts: string;
}

interface Message {
  id: string;
  content: string;
  direction: string;
  ts: string;
}

interface DeploymentData {
  deployment: {
    id: string;
    status: string;
    strategyId: string;
    hotAddress: string;
    createdAt: string;
    config?: Record<string, unknown>;
  };
  trades: Trade[];
  messages: Message[];
}

function formatUptime(createdAt: string): string {
  const diff = Math.max(0, Math.floor((Date.now() - new Date(createdAt).getTime()) / 1000));
  const h = Math.floor(diff / 3600);
  const m = Math.floor((diff % 3600) / 60);
  const s = diff % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function formatStrategy(strategyId: string): string {
  return strategyId
    .replace(/[-_]/g, " ")
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

function truncateAddress(addr: string): string {
  if (!addr || addr.length < 10) return addr ?? "—";
  return `${addr.slice(0, 6)}...${addr.slice(-4)}`;
}

function statusColor(status: string): string {
  switch (status) {
    case "active":
      return "bg-success";
    case "pending":
    case "terminating":
      return "bg-yellow-400";
    case "terminated":
    case "failed":
      return "bg-danger";
    default:
      return "bg-text-muted";
  }
}

function statusLabel(status: string): string {
  switch (status) {
    case "active":
      return "Agent Running";
    case "pending":
      return "Starting Up";
    case "terminating":
      return "Shutting Down";
    case "terminated":
      return "Terminated";
    case "failed":
      return "Failed";
    default:
      return status;
  }
}

const demoTrades: Trade[] = [
  { id: "1", venue: "Polymarket", action: "BUY", amountUsd: "10.00", pnlUsd: "3.20", ts: new Date().toISOString() },
  { id: "2", venue: "Polymarket", action: "SELL", amountUsd: "8.50", pnlUsd: "1.87", ts: new Date().toISOString() },
  { id: "3", venue: "Hyperliquid", action: "LONG", amountUsd: "15.00", pnlUsd: "-0.43", ts: new Date().toISOString() },
  { id: "4", venue: "Polymarket", action: "BUY", amountUsd: "12.00", pnlUsd: "7.83", ts: new Date().toISOString() },
];

export default function DashboardPage() {
  const searchParams = useSearchParams();
  const deploymentId = searchParams.get("deploymentId");

  const [data, setData] = useState<DeploymentData | null>(null);
  const [localMessages, setLocalMessages] = useState<Message[]>([]);
  const [chatInput, setChatInput] = useState("");
  const [isTerminating, setIsTerminating] = useState(false);
  const [uptime, setUptime] = useState("00:00:00");
  const [fetchError, setFetchError] = useState(false);

  const chatEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // --- Fetch deployment data with 5s polling ---
  const fetchData = useCallback(async () => {
    if (!deploymentId) return;
    try {
      const res = await fetch(`/api/agent?deploymentId=${deploymentId}`);
      if (!res.ok) {
        setFetchError(true);
        return;
      }
      const json: DeploymentData = await res.json();
      setData(json);
      setFetchError(false);
      if (json.deployment.status === "terminated" || json.deployment.status === "terminating") {
        setIsTerminating(true);
      }
    } catch {
      setFetchError(true);
    }
  }, [deploymentId]);

  useEffect(() => {
    if (!deploymentId) return;
    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [deploymentId, fetchData]);

  // --- Uptime ticker (1s) ---
  useEffect(() => {
    if (!data?.deployment?.createdAt) return;
    const tick = () => setUptime(formatUptime(data.deployment.createdAt));
    tick();
    const interval = setInterval(tick, 1000);
    return () => clearInterval(interval);
  }, [data?.deployment?.createdAt]);

  // --- Auto-scroll chat ---
  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [data?.messages, localMessages]);

  // --- Kill Switch ---
  const handleKillSwitch = async () => {
    if (!deploymentId) return;
    if (!confirm("Are you sure? This will close all positions and return funds.")) return;

    try {
      const res = await fetch("/api/deploy/terminate", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ deploymentId }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        alert(err.error ?? "Failed to terminate deployment.");
        return;
      }
      setIsTerminating(true);
      fetchData();
    } catch {
      alert("Network error. Could not reach the server.");
    }
  };

  // --- Send Chat ---
  const handleSendChat = async () => {
    if (!deploymentId || !chatInput.trim()) return;
    const content = chatInput.trim();

    // Optimistic local message
    const optimistic: Message = {
      id: `local-${Date.now()}`,
      content,
      direction: "user_to_agent",
      ts: new Date().toISOString(),
    };
    setLocalMessages((prev) => [...prev, optimistic]);
    setChatInput("");

    try {
      const res = await fetch("/api/agent/command", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ deploymentId, content }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        alert(err.error ?? "Failed to send message.");
        // Remove optimistic message on failure
        setLocalMessages((prev) => prev.filter((m) => m.id !== optimistic.id));
      }
    } catch {
      alert("Network error. Could not send message.");
      setLocalMessages((prev) => prev.filter((m) => m.id !== optimistic.id));
    }
  };

  // --- First trade confetti ---
  useFirstTradeConfetti(data?.trades ?? []);

  // --- Derived state ---
  const status = data?.deployment?.status ?? "pending";
  const isTerminated = status === "terminated" || status === "failed";
  const trades = data?.trades?.length ? data.trades : demoTrades;
  const totalPnl = data?.trades?.length
    ? data.trades.reduce((sum, t) => sum + parseFloat(t.pnlUsd || "0"), 0)
    : 12.47;

  // Merge server messages (agent_to_user) with local user messages, sorted by time
  const allMessages: Message[] = [
    ...(data?.messages ?? []),
    ...localMessages,
  ].sort((a, b) => new Date(a.ts).getTime() - new Date(b.ts).getTime());

  // --- No deployment selected ---
  if (!deploymentId) {
    return (
      <div className="min-h-screen pt-14 flex items-center justify-center">
        <div className="text-center space-y-3">
          <div className="text-2xl font-semibold">No Deployment Selected</div>
          <div className="text-sm text-text-muted">
            Deploy an agent first, then return here with a deployment ID.
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-screen pt-14">
      <div className="flex h-[calc(100vh-56px)]">
        {/* Sidebar */}
        <aside className="w-[240px] border-r border-border bg-surface p-6 flex flex-col gap-6 shrink-0">
          {/* Status */}
          <motion.div
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            className="flex items-center gap-2 rounded-sm bg-accent-subtle border border-accent/15 px-4 py-3"
          >
            <span
              className={`w-2 h-2 rounded-full ${statusColor(status)} ${
                status === "active" ? "animate-pulse" : ""
              }`}
            />
            <span className="text-[13px] font-semibold">{statusLabel(status)}</span>
          </motion.div>

          <div>
            <div className="text-xs text-text-muted mb-1">Strategy</div>
            <div className="text-sm font-semibold">
              {data?.deployment?.strategyId
                ? formatStrategy(data.deployment.strategyId)
                : "—"}
            </div>
          </div>

          <div>
            <div className="text-xs text-text-muted mb-1">Uptime</div>
            <div className="font-mono text-sm">{isTerminated ? "—" : uptime}</div>
          </div>

          <div>
            <div className="text-xs text-text-muted mb-1">Hot Address</div>
            <div className="font-mono text-sm" title={data?.deployment?.hotAddress}>
              {data?.deployment?.hotAddress
                ? truncateAddress(data.deployment.hotAddress)
                : "—"}
            </div>
          </div>

          {fetchError && (
            <div className="text-xs text-danger">Connection lost. Retrying...</div>
          )}

          {/* Kill Switch */}
          <button
            onClick={handleKillSwitch}
            disabled={isTerminating || isTerminated}
            className="mt-auto py-3 text-center bg-danger/10 text-danger border border-danger/20 rounded-sm font-semibold text-sm transition-all hover:bg-danger/20 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {isTerminated
              ? "Terminated"
              : isTerminating
                ? "Shutting down..."
                : "Kill Switch"}
          </button>
        </aside>

        {/* Main */}
        <main className="flex-1 p-6 overflow-y-auto space-y-4">
          {/* P&L */}
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.05 }}
            className="bg-surface rounded-md p-6"
          >
            <div className="text-[13px] text-text-muted mb-2">Total P&L</div>
            <div
              className={`font-mono text-4xl font-semibold tabular-nums ${
                totalPnl >= 0 ? "text-success" : "text-danger"
              }`}
            >
              {totalPnl >= 0 ? "+" : ""}${totalPnl.toFixed(2)}
            </div>
          </motion.div>

          {/* Trade Log */}
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.1 }}
            className="bg-surface rounded-md p-4"
          >
            <div className="text-[13px] font-semibold text-text-muted mb-3">
              Recent Trades
              {!data?.trades?.length && (
                <span className="ml-2 text-xs font-normal opacity-60">(demo)</span>
              )}
            </div>
            <AnimatePresence initial={false}>
              {trades.map((trade) => (
                <motion.div
                  key={trade.id}
                  initial={{ opacity: 0, x: -8 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0 }}
                  className="grid grid-cols-[auto_1fr_auto_auto] gap-4 py-2.5 border-b border-border last:border-0 text-[13px] items-center"
                >
                  <span className="font-mono text-xs text-text-muted">
                    {new Date(trade.ts).toLocaleTimeString("en-US", { hour12: false })}
                  </span>
                  <span className="font-medium">{trade.venue}</span>
                  <span
                    className={`font-mono text-xs ${
                      trade.action === "SELL" ? "text-danger" : "text-success"
                    }`}
                  >
                    {trade.action}
                  </span>
                  <span
                    className={`font-mono font-semibold text-[13px] ${
                      parseFloat(trade.pnlUsd) >= 0 ? "text-success" : "text-danger"
                    }`}
                  >
                    {parseFloat(trade.pnlUsd) >= 0 ? "+" : ""}${trade.pnlUsd}
                  </span>
                </motion.div>
              ))}
            </AnimatePresence>
          </motion.div>

          {/* Chat */}
          <motion.div
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.15 }}
            className={`bg-surface rounded-md p-4 ${isTerminated ? "opacity-50" : ""}`}
          >
            <div className="text-[13px] font-semibold text-text-muted mb-3">
              Chat with Agent
            </div>

            {/* Messages area */}
            <div className="min-h-[120px] max-h-[280px] overflow-y-auto mb-3 space-y-2 scrollbar-thin">
              {allMessages.length === 0 ? (
                <div className="text-sm text-text-muted py-4 text-center">
                  {isTerminated
                    ? "Agent terminated. Chat is read-only."
                    : "Say hi to your agent!"}
                </div>
              ) : (
                <AnimatePresence initial={false}>
                  {allMessages.map((msg) => {
                    const isUser = msg.direction === "user_to_agent";
                    return (
                      <motion.div
                        key={msg.id}
                        initial={{ opacity: 0, y: 4 }}
                        animate={{ opacity: 1, y: 0 }}
                        className={`flex ${isUser ? "justify-end" : "justify-start"}`}
                      >
                        <div
                          className={`max-w-[75%] px-3 py-2 rounded-md text-sm ${
                            isUser
                              ? "bg-accent/15 text-accent"
                              : "bg-bg border border-border"
                          }`}
                        >
                          <div>{msg.content}</div>
                          <div className="text-[10px] text-text-muted mt-1 font-mono">
                            {new Date(msg.ts).toLocaleTimeString("en-US", {
                              hour12: false,
                            })}
                          </div>
                        </div>
                      </motion.div>
                    );
                  })}
                </AnimatePresence>
              )}
              <div ref={chatEndRef} />
            </div>

            {/* Input */}
            <div className="flex gap-2">
              <input
                ref={inputRef}
                value={chatInput}
                onChange={(e) => setChatInput(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && handleSendChat()}
                placeholder={
                  isTerminated
                    ? "Agent is terminated"
                    : "Ask your agent anything..."
                }
                maxLength={500}
                disabled={isTerminated}
                className="flex-1 px-3.5 py-2.5 bg-bg border border-border rounded-sm text-sm outline-none focus:border-accent transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
              />
              <button
                onClick={handleSendChat}
                disabled={isTerminated || !chatInput.trim()}
                className="px-5 py-2.5 bg-accent text-bg font-semibold text-sm rounded-sm hover:bg-accent-hover transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Send
              </button>
            </div>
          </motion.div>
        </main>
      </div>
    </div>
  );
}
