"use client";

import { useEffect, useState, useRef, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { useSearchParams, usePathname } from "next/navigation";
import Link from "next/link";
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

function statusColor(status: string, phase?: string, unhealthy?: boolean, gatewayAlive?: boolean): string {
  if (unhealthy) return "bg-warning";
  switch (status) {
    case "active":
      if (phase === "trading" || phase === "ready") return "bg-success";
      return gatewayAlive ? "bg-success" : "bg-accent";
    case "pending":
    case "sdl_generated":
    case "awaiting_bids":
    case "selecting_provider":
    case "terminating":
      return "bg-accent";
    case "terminated":
    case "failed":
      return "bg-danger";
    default:
      return "bg-text-muted";
  }
}

function statusLabel(status: string, phase?: string, unhealthy?: boolean, gatewayAlive?: boolean): string {
  if (unhealthy) return "Unhealthy";
  if (status === "active") {
    switch (phase) {
      case "bootstrap": return "Creating Wallet...";
      case "ready": return "Agent Ready";
      case "trading": return "Trading";
      default: return gatewayAlive ? "Agent Running" : "Initializing...";
    }
  }
  switch (status) {
    case "pending": return "Creating...";
    case "sdl_generated": return "Config Built";
    case "awaiting_bids": return "Finding Providers";
    case "selecting_provider": return "Selecting Provider";
    case "terminating": return "Shutting Down";
    case "terminated": return "Terminated";
    case "failed": return "Failed";
    default: return status;
  }
}

const DEPLOY_STEPS = [
  { key: "pending", label: "Create", desc: "Initializing deployment" },
  { key: "sdl_generated", label: "Build", desc: "Configuration generated" },
  { key: "awaiting_bids", label: "Discover", desc: "Searching Akash providers" },
  { key: "selecting_provider", label: "Select", desc: "Choosing best provider" },
  { key: "active", label: "Live", desc: "Agent is running" },
] as const;

const DEPLOY_ORDER = DEPLOY_STEPS.map((s) => s.key);

function isDeploying(status: string): boolean {
  return DEPLOY_ORDER.includes(status as any) && status !== "active";
}

function DeployProgress({ status, bidCount }: { status: string; bidCount?: number }) {
  const currentIdx = DEPLOY_ORDER.indexOf(status as any);

  return (
    <div className="bg-surface rounded-md p-6 space-y-4">
      <div className="text-[13px] font-semibold text-text-muted">Deploying to Akash Network</div>
      <div className="flex items-center gap-1">
        {DEPLOY_STEPS.map((step, i) => {
          const done = i < currentIdx;
          const active = i === currentIdx;
          return (
            <div key={step.key} className="flex-1 flex flex-col items-center gap-2">
              <div className="w-full flex items-center">
                <motion.div
                  className={`w-6 h-6 rounded-full flex items-center justify-center text-[10px] font-bold shrink-0 ${
                    done
                      ? "bg-accent text-bg"
                      : active
                        ? "bg-accent/20 text-accent border-2 border-accent"
                        : "bg-border text-text-muted"
                  }`}
                  animate={active ? { scale: [1, 1.15, 1] } : {}}
                  transition={active ? { repeat: Infinity, duration: 1.5 } : {}}
                >
                  {done ? "✓" : i + 1}
                </motion.div>
                {i < DEPLOY_STEPS.length - 1 && (
                  <div className={`flex-1 h-[2px] mx-1 ${done ? "bg-accent" : "bg-border"}`} />
                )}
              </div>
              <div className="text-center">
                <div className={`text-[11px] font-semibold ${active ? "text-accent" : done ? "text-text-muted" : "text-text-muted/50"}`}>
                  {step.label}
                </div>
              </div>
            </div>
          );
        })}
      </div>
      <div className="text-center">
        <div className="text-sm text-accent font-medium">
          {DEPLOY_STEPS[currentIdx]?.desc ?? "Processing..."}
        </div>
        {status === "awaiting_bids" && bidCount !== undefined && bidCount > 0 && (
          <div className="text-xs text-text-muted mt-1 font-mono">{bidCount} providers found</div>
        )}
      </div>
    </div>
  );
}

const demoTrades: Trade[] = [
  { id: "1", venue: "Polymarket", action: "BUY", amountUsd: "10.00", pnlUsd: "3.20", ts: new Date().toISOString() },
  { id: "2", venue: "Polymarket", action: "SELL", amountUsd: "8.50", pnlUsd: "1.87", ts: new Date().toISOString() },
  { id: "3", venue: "Hyperliquid", action: "LONG", amountUsd: "15.00", pnlUsd: "-0.43", ts: new Date().toISOString() },
  { id: "4", venue: "Polymarket", action: "BUY", amountUsd: "12.00", pnlUsd: "7.83", ts: new Date().toISOString() },
];

export default function DashboardPage() {
  const searchParams = useSearchParams();
  const pathname = usePathname();
  const locale = pathname.split("/")[1] || "en";
  const deploymentId = searchParams.get("deploymentId");

  const [data, setData] = useState<DeploymentData | null>(null);
  const [localMessages, setLocalMessages] = useState<Message[]>([]);
  const [chatInput, setChatInput] = useState("");
  const [isTerminating, setIsTerminating] = useState(false);
  const [uptime, setUptime] = useState("00:00:00");
  const [fetchError, setFetchError] = useState(false);
  const [bidCount, setBidCount] = useState(0);
  const [prevTradeIds, setPrevTradeIds] = useState<Set<string>>(new Set());
  const [newTradeIds, setNewTradeIds] = useState<Set<string>>(new Set());

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

  // --- Deploy progress polling (fast, 2s) when deploying ---
  useEffect(() => {
    const status = data?.deployment?.status;
    if (!deploymentId || !status || !isDeploying(status)) return;

    const pollProgress = async () => {
      try {
        const res = await fetch(`/api/deploy/progress?deploymentId=${deploymentId}`);
        if (!res.ok) return;
        const json = await res.json();
        if (json.bidCount) setBidCount(json.bidCount);
        // Refresh main data if status changed
        if (json.status !== status) fetchData();
      } catch { /* retry next tick */ }
    };

    pollProgress();
    const interval = setInterval(pollProgress, 2000);
    return () => clearInterval(interval);
  }, [deploymentId, data?.deployment?.status, fetchData]);

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

  // --- Track new trades for flash animation ---
  useEffect(() => {
    if (!data?.trades?.length) return;
    const currentIds = new Set(data.trades.map((t) => t.id));
    const fresh = data.trades.filter((t) => !prevTradeIds.has(t.id)).map((t) => t.id);
    if (fresh.length > 0 && prevTradeIds.size > 0) {
      setNewTradeIds(new Set(fresh));
      setTimeout(() => setNewTradeIds(new Set()), 2000);
    }
    setPrevTradeIds(currentIds);
  }, [data?.trades]);

  // --- Gateway health polling (after active, before heartbeat) ---
  const [gatewayAlive, setGatewayAlive] = useState(false);

  useEffect(() => {
    const status = data?.deployment?.status;
    const config = data?.deployment?.config as Record<string, any> | undefined;
    const gwUrl = config?._gatewayUrl;
    if (status !== "active" || !gwUrl || gatewayAlive) return;

    const checkGateway = async () => {
      try {
        const res = await fetch(gwUrl, { mode: "no-cors", signal: AbortSignal.timeout(5000) });
        setGatewayAlive(true);
      } catch { /* not ready yet */ }
    };

    checkGateway();
    const interval = setInterval(checkGateway, 10000);
    return () => clearInterval(interval);
  }, [data?.deployment?.status, data?.deployment?.config, gatewayAlive]);

  // --- Derived state ---
  const status = data?.deployment?.status ?? "pending";
  const isTerminated = status === "terminated" || status === "failed";
  const config = data?.deployment?.config as Record<string, any> | undefined;
  const gatewayUrl = config?._gatewayUrl as string | undefined;
  const phase = config?._phase as string | undefined;
  const lastHeartbeat = config?._lastHeartbeat as string | undefined;
  const unhealthy = status === "active" && lastHeartbeat
    ? (Date.now() - new Date(lastHeartbeat).getTime()) > 60000
    : false;
  const trades = data?.trades?.length ? data.trades : demoTrades;
  const totalPnl = data?.trades?.length
    ? data.trades.reduce((sum, t) => sum + parseFloat(t.pnlUsd || "0"), 0)
    : 12.47;

  // Merge server messages with local optimistic messages, deduplicate by content+time
  const serverMessages = data?.messages ?? [];
  const serverContentSet = new Set(serverMessages.map((m) => m.content + m.direction));
  const uniqueLocal = localMessages.filter(
    (m) => !serverContentSet.has(m.content + m.direction)
  );
  const allMessages: Message[] = [
    ...serverMessages,
    ...uniqueLocal,
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
    <div className="min-h-screen">
      {/* Nav */}
      <nav className="fixed top-0 left-0 right-0 z-50 border-b border-border bg-bg/80 backdrop-blur-sm">
        <div className="mx-auto max-w-[1440px] px-6 flex items-center justify-between h-14">
          <Link href={`/${locale}`} className="text-lg font-bold tracking-tight hover:text-accent transition">
            a2ex<span className="text-accent">.</span>
          </Link>
          <div className="flex items-center gap-4 text-[13px]">
            <span className="text-text-muted">Dashboard</span>
            <Link href={`/${locale}`} className="text-text-muted hover:text-accent transition">
              Deploy New
            </Link>
          </div>
        </div>
      </nav>
      <div className="flex h-[calc(100vh-56px)] pt-14">
        {/* Sidebar */}
        <aside className="w-[240px] border-r border-border bg-surface p-6 flex flex-col gap-6 shrink-0">
          {/* Status */}
          <motion.div
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            className="flex items-center gap-2 rounded-sm bg-accent-subtle border border-accent/15 px-4 py-3"
          >
            <span
              className={`w-2 h-2 rounded-full ${statusColor(status, phase, unhealthy, gatewayAlive)} ${
                status === "active" && !unhealthy ? "animate-pulse" : ""
              }`}
            />
            <span className="text-[13px] font-semibold">{statusLabel(status, phase, unhealthy, gatewayAlive)}</span>
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

          {gatewayUrl && (
            <div>
              <div className="text-xs text-text-muted mb-1">Gateway</div>
              <div className="flex items-center gap-1.5">
                <span className={`w-1.5 h-1.5 rounded-full ${gatewayAlive ? "bg-success" : "bg-text-muted"}`} />
                <span className="font-mono text-xs truncate" title={gatewayUrl}>
                  {gatewayAlive ? "Connected" : "Starting..."}
                </span>
              </div>
            </div>
          )}

          {unhealthy && (
            <div className="text-xs text-warning">
              Agent not responding. Last seen: {lastHeartbeat ? `${Math.round((Date.now() - new Date(lastHeartbeat).getTime()) / 1000)}s ago` : "never"}
            </div>
          )}
          {fetchError && !unhealthy && (
            <div className="text-xs text-text-muted">Reconnecting...</div>
          )}

          {/* Kill Switch */}
          <button
            onClick={handleKillSwitch}
            disabled={isTerminating || isTerminated || isDeploying(status)}
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
          {/* Deploy Progress (shown during deployment) */}
          {isDeploying(status) && (
            <motion.div
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
            >
              <DeployProgress status={status} bidCount={bidCount} />
            </motion.div>
          )}

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
                  animate={{ opacity: 1, x: 0, backgroundColor: newTradeIds.has(trade.id) ? "rgba(240, 160, 48, 0.15)" : "transparent" }}
                  exit={{ opacity: 0 }}
                  transition={{ backgroundColor: { duration: 2 } }}
                  className="grid grid-cols-[auto_1fr_auto_auto] gap-4 py-2.5 border-b border-border last:border-0 text-[13px] items-center rounded-sm"
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
                    : isDeploying(status)
                      ? "Agent is starting up..."
                      : phase === "ready"
                        ? "Ask about the market, your strategy, or recent trades..."
                        : "Ask your agent anything..."
                }
                maxLength={500}
                disabled={isTerminated || isDeploying(status)}
                className="flex-1 px-3.5 py-2.5 bg-bg border border-border rounded-sm text-sm outline-none focus:border-accent transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
              />
              <button
                onClick={handleSendChat}
                disabled={isTerminated || isDeploying(status) || !chatInput.trim()}
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
