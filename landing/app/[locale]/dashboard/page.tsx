"use client";

import { useEffect, useState, useRef, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { useSearchParams, usePathname } from "next/navigation";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useFirstTradeConfetti } from "@/lib/hooks/useFirstTradeConfetti";
import { useAuth } from "@/lib/hooks/useAuth";
import FundingModal from "@/components/FundingModal";

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
  return strategyId.replace(/[-_]/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
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
    case "creating_lease":
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
    case "creating_lease": return "Creating Lease";
    case "terminating": return "Shutting Down";
    case "terminated": return "Terminated";
    case "failed": return "Failed";
    default: return status;
  }
}

const DEPLOY_STEPS = [
  { key: "pending", label: "Create", desc: "Initializing deployment" },
  { key: "sdl_generated", label: "Build", desc: "Configuration generated" },
  { key: "awaiting_bids", label: "Discover", desc: "Searching providers" },
  { key: "selecting_provider", label: "Select", desc: "Choosing best provider" },
  { key: "creating_lease", label: "Lease", desc: "Creating lease" },
  { key: "active", label: "Live", desc: "Agent is running" },
] as const;

const DEPLOY_ORDER = DEPLOY_STEPS.map((s) => s.key);

function isDeploying(status: string): boolean {
  return DEPLOY_ORDER.includes(status as any) && status !== "active";
}

function DeployProgress({ status, bidCount }: { status: string; bidCount?: number }) {
  const currentIdx = DEPLOY_ORDER.indexOf(status as any);
  return (
    <div className="bg-surface rounded-md p-4 sm:p-6 space-y-4" role="progressbar" aria-valuenow={currentIdx} aria-valuemin={0} aria-valuemax={DEPLOY_STEPS.length - 1} aria-label="Deployment progress">
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
                    done ? "bg-accent text-bg" : active ? "bg-accent/20 text-accent border-2 border-accent" : "bg-border text-text-muted"
                  }`}
                  animate={active ? { scale: [1, 1.15, 1] } : {}}
                  transition={active ? { repeat: Infinity, duration: 1.5 } : {}}
                  aria-label={`Step ${i + 1}: ${step.label} — ${done ? "complete" : active ? "in progress" : "pending"}`}
                >
                  {done ? "✓" : i + 1}
                </motion.div>
                {i < DEPLOY_STEPS.length - 1 && (
                  <div className={`flex-1 h-[2px] mx-1 ${done ? "bg-accent" : "bg-border"}`} />
                )}
              </div>
              <div className="text-center hidden sm:block">
                <div className={`text-[11px] font-semibold ${active ? "text-accent" : done ? "text-text-muted" : "text-text-muted/50"}`}>
                  {step.label}
                </div>
              </div>
            </div>
          );
        })}
      </div>
      <div className="text-center">
        <div className="text-sm text-accent font-medium">{DEPLOY_STEPS[currentIdx]?.desc ?? "Processing..."}</div>
        {(status === "awaiting_bids" || status === "selecting_provider") && bidCount !== undefined && bidCount > 0 && (
          <div className="text-xs text-text-muted mt-1 font-mono">{bidCount} providers found</div>
        )}
      </div>
    </div>
  );
}

function FailedRecovery({ error, deploymentId, locale }: { error?: string; deploymentId: string; locale: string }) {
  return (
    <div className="bg-surface rounded-md p-6 space-y-4 border border-danger/20" role="alert">
      <div className="flex items-center gap-2">
        <span className="w-2.5 h-2.5 rounded-full bg-danger" />
        <span className="text-sm font-semibold text-danger">Deployment Failed</span>
      </div>
      {error && <div className="text-sm text-text-muted bg-bg rounded p-3 font-mono text-xs break-all">{error}</div>}
      <div className="text-sm text-text-muted">
        Your $5 deposit has been automatically returned to the Akash escrow wallet.
        No funds were lost.
      </div>
      <div className="flex gap-3">
        <Link
          href={`/${locale}`}
          className="flex-1 py-2.5 text-center bg-accent text-bg font-semibold text-sm rounded-sm hover:bg-accent-hover transition-colors min-h-[44px] flex items-center justify-center"
        >
          Deploy New Agent
        </Link>
        <button
          onClick={() => window.location.reload()}
          className="px-4 py-2.5 border border-border text-sm rounded-sm hover:bg-surface transition-colors min-h-[44px]"
        >
          Retry
        </button>
      </div>
    </div>
  );
}

export default function DashboardPage() {
  const searchParams = useSearchParams();
  const pathname = usePathname();
  const locale = pathname.split("/")[1] || "en";
  const deploymentId = searchParams.get("deploymentId");

  const [data, setData] = useState<DeploymentData | null>(null);
  const [localMessages, setLocalMessages] = useState<Message[]>([]);
  const [chatInput, setChatInput] = useState("");
  const [isTerminating, setIsTerminating] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [showStopConfirm, setShowStopConfirm] = useState(false);
  const [uptime, setUptime] = useState("00:00:00");
  const [fetchError, setFetchError] = useState(false);
  const [bidCount, setBidCount] = useState(0);
  const [sseStatus, setSseStatus] = useState<string | undefined>();
  const [sseError, setSseError] = useState<string | undefined>();
  const [prevTradeIds, setPrevTradeIds] = useState<Set<string>>(new Set());
  const [newTradeIds, setNewTradeIds] = useState<Set<string>>(new Set());
  const [sidebarOpen, setSidebarOpen] = useState(false);

  const chatEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const sseRef = useRef<EventSource | null>(null);
  const router = useRouter();
  const { authenticate, isAuthenticating } = useAuth();
  const [isRecovering, setIsRecovering] = useState(false);
  const [showFunding, setShowFunding] = useState(false);

  // --- Fetch deployment data (one-shot hydration + Phase 2 polling) ---
  const fetchData = useCallback(async () => {
    if (!deploymentId) return;
    try {
      const res = await fetch(`/api/agent?deploymentId=${deploymentId}`);
      if (!res.ok) { setFetchError(true); return; }
      const json: DeploymentData = await res.json();
      setData(json);
      setFetchError(false);
      if (json.deployment.status === "terminated" || json.deployment.status === "terminating") {
        setIsTerminating(true);
      }
    } catch { setFetchError(true); }
  }, [deploymentId]);

  // Initial hydration
  useEffect(() => {
    fetchData();
  }, [fetchData]);

  // Phase 2 polling (only when active, 5s interval)
  useEffect(() => {
    const status = data?.deployment?.status;
    if (!deploymentId || !status) return;
    if (isDeploying(status)) return; // SSE handles deploying states
    if (status === "terminated" || status === "failed") return; // no need to poll

    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [deploymentId, data?.deployment?.status, fetchData]);

  // --- Phase 1 SSE (deploy lifecycle only) ---
  useEffect(() => {
    const status = data?.deployment?.status;
    if (!deploymentId || !status || !isDeploying(status)) {
      if (sseRef.current) { sseRef.current.close(); sseRef.current = null; }
      return;
    }
    if (sseRef.current) return; // don't open duplicate

    const es = new EventSource(`/api/deploy/stream?deploymentId=${deploymentId}`);
    sseRef.current = es;

    es.addEventListener("status", (e) => {
      try {
        const d = JSON.parse(e.data);
        if (d.status) setSseStatus(d.status);
        if (d.bidCount) setBidCount(d.bidCount);
      } catch {}
    });

    es.addEventListener("bids", (e) => {
      try {
        const d = JSON.parse(e.data);
        if (d.bidCount != null) setBidCount(d.bidCount);
      } catch {}
    });

    es.addEventListener("active", () => {
      fetchData(); // refresh full data
      if (sseRef.current) { sseRef.current.close(); sseRef.current = null; }
    });

    es.addEventListener("failed", (e) => {
      try { const d = JSON.parse(e.data); setSseError(d.error); } catch {}
      fetchData();
      if (sseRef.current) { sseRef.current.close(); sseRef.current = null; }
    });

    es.onerror = () => {
      if (sseRef.current) { sseRef.current.close(); sseRef.current = null; }
      fetchData(); // check current state
    };

    return () => {
      if (sseRef.current) { sseRef.current.close(); sseRef.current = null; }
    };
  }, [deploymentId, data?.deployment?.status, fetchData]);

  // --- Uptime ticker ---
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

  // --- Cancel Deploy (during Phase 1) ---
  const handleCancelDeploy = async () => {
    if (!deploymentId) return;
    if (!confirm("Cancel this deployment? The deposit will be returned.")) return;
    try {
      const res = await fetch("/api/deploy/terminate", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ deploymentId }),
      });
      if (res.ok) { setIsTerminating(true); fetchData(); }
    } catch {}
  };

  // --- Pause/Resume Trading ---
  const handlePauseResume = async () => {
    if (!deploymentId) return;
    const command = isPaused ? "SYSTEM:RESUME" : "SYSTEM:PAUSE";
    try {
      const res = await fetch("/api/agent/command", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ deploymentId, content: command }),
      });
      if (res.ok) setIsPaused(!isPaused);
    } catch { alert("Network error."); }
  };

  // --- Stop Agent (full shutdown) ---
  const handleStopAgent = async () => {
    if (!deploymentId) return;
    setShowStopConfirm(false);
    try {
      const res = await fetch("/api/deploy/terminate", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ deploymentId }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({})) as Record<string, string>;
        alert(err.error ?? "Failed to stop.");
        return;
      }
      setIsTerminating(true);
      fetchData();
    } catch { alert("Network error."); }
  };

  // --- Send Chat (SSE streaming) ---
  const [isStreaming, setIsStreaming] = useState(false);
  const [streamingText, setStreamingText] = useState("");

  const handleSendChat = async () => {
    if (!deploymentId || !chatInput.trim() || isStreaming) return;
    const content = chatInput.trim();
    const userMsg: Message = { id: `local-${Date.now()}`, content, direction: "user_to_agent", ts: new Date().toISOString() };
    setLocalMessages((prev) => [...prev, userMsg]);
    setChatInput("");
    setIsStreaming(true);
    setStreamingText("");

    // Create placeholder for agent response
    const agentMsgId = `stream-${Date.now()}`;

    try {
      const res = await fetch("/api/agent/chat", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ deploymentId, content }),
      });

      if (!res.ok || !res.body) {
        // Fallback to old command API
        await fetch("/api/agent/command", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ deploymentId, content }),
        });
        setIsStreaming(false);
        return;
      }

      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let fullText = "";

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        const chunk = decoder.decode(value, { stream: true });
        const lines = chunk.split("\n");

        for (const line of lines) {
          if (line.startsWith("data: ")) {
            try {
              const data = JSON.parse(line.slice(6));
              if (data.token) {
                fullText += data.token;
                setStreamingText(fullText);
              }
              if (data.fullText !== undefined) {
                fullText = data.fullText || fullText;
              }
            } catch {}
          }
        }
      }

      // Add final agent message
      if (fullText) {
        setLocalMessages((prev) => [...prev, {
          id: agentMsgId,
          content: fullText,
          direction: "agent_to_user",
          ts: new Date().toISOString(),
        }]);
      }
    } catch {
      // Fallback to command API on error
      await fetch("/api/agent/command", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ deploymentId, content }),
      }).catch(() => {});
    } finally {
      setIsStreaming(false);
      setStreamingText("");
    }
  };

  // --- Confetti + trade flash ---
  useFirstTradeConfetti(data?.trades ?? []);
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

  // --- Derived state ---
  const displayStatus = sseStatus ?? data?.deployment?.status ?? "pending";
  const status = data?.deployment?.status ?? "pending";
  const isTerminated = status === "terminated" || status === "failed";
  const config = data?.deployment?.config as Record<string, any> | undefined;
  const usdcBalance = config?._usdcBalance as string | undefined;
  const lastBackupAt = config?._lastBackupAt as string | undefined;
  const gatewayUrl = config?._gatewayUrl as string | undefined;
  const phase = config?._phase as string | undefined;
  const lastHeartbeat = config?._lastHeartbeat as string | undefined;
  const gatewayAlive = phase === "ready" || phase === "trading" || phase === "bootstrap";
  const agentReady = phase === "ready" || phase === "trading";
  const unhealthy = status === "active" && lastHeartbeat
    ? (Date.now() - new Date(lastHeartbeat).getTime()) > 60000
    : false;
  const hasBackup = !!(data?.deployment as any)?.encryptedBackup;
  const showDemo = status === "active" && gatewayAlive && !data?.trades?.length;
  const trades = data?.trades?.length ? data.trades : [];
  const totalPnl = data?.trades?.length
    ? data.trades.reduce((sum, t) => sum + parseFloat(t.pnlUsd || "0"), 0)
    : 0;

  const serverMessages = data?.messages ?? [];
  const serverContentSet = new Set(serverMessages.map((m) => m.content + m.direction));
  const uniqueLocal = localMessages.filter((m) => !serverContentSet.has(m.content + m.direction));
  const allMessages: Message[] = [...serverMessages, ...uniqueLocal]
    .sort((a, b) => new Date(a.ts).getTime() - new Date(b.ts).getTime());

  const handleRecover = async () => {
    if (!deploymentId || isRecovering) return;
    setIsRecovering(true);
    try {
      const authed = await authenticate();
      if (!authed) { alert("Authentication failed."); return; }

      const res = await fetch("/api/deploy", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          strategyId: data?.deployment?.strategyId || "sports-basic",
          config: { fundAmountUsd: 10, riskLevel: "low" },
          recoveryFromId: deploymentId,
        }),
      });
      const newDep = await res.json();
      if (newDep.id) {
        router.push(`/${locale}/dashboard?deploymentId=${newDep.id}`);
      } else {
        alert(newDep.error ?? "Recovery failed.");
      }
    } catch { alert("Network error."); } finally { setIsRecovering(false); }
  };

  if (!deploymentId) {
    return (
      <div className="min-h-screen pt-14 flex items-center justify-center">
        <div className="text-center space-y-3 px-6">
          <div className="text-2xl font-semibold">No Deployment Selected</div>
          <div className="text-sm text-text-muted">Deploy an agent first, then return here.</div>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-screen">
      {/* Nav */}
      <nav className="fixed top-0 left-0 right-0 z-50 border-b border-border bg-bg/80 backdrop-blur-sm" role="navigation" aria-label="Main navigation">
        <div className="mx-auto max-w-[1440px] px-4 sm:px-6 flex items-center justify-between h-14">
          <Link href={`/${locale}`} className="text-lg font-bold tracking-tight hover:text-accent transition min-h-[44px] flex items-center">
            a2ex<span className="text-accent">.</span>
          </Link>
          <div className="flex items-center gap-4 text-[13px]">
            {/* Mobile sidebar toggle */}
            <button
              onClick={() => setSidebarOpen(!sidebarOpen)}
              className="sm:hidden min-h-[44px] min-w-[44px] flex items-center justify-center"
              aria-label="Toggle sidebar"
            >
              <span className={`w-2 h-2 rounded-full ${statusColor(isDeploying(status) ? displayStatus : status, phase, unhealthy, gatewayAlive)}`} />
            </button>
            <span className="text-text-muted hidden sm:inline">Dashboard</span>
            <Link href={`/${locale}`} className="text-text-muted hover:text-accent transition min-h-[44px] flex items-center">
              Deploy New
            </Link>
          </div>
        </div>
      </nav>

      <div className="flex flex-col sm:flex-row h-[calc(100vh-56px)] pt-14">
        {/* Sidebar — hidden on mobile unless toggled */}
        <aside
          className={`${sidebarOpen ? "block" : "hidden"} sm:block w-full sm:w-[240px] border-b sm:border-b-0 sm:border-r border-border bg-surface p-4 sm:p-6 flex flex-col gap-4 sm:gap-6 shrink-0`}
          role="complementary"
          aria-label="Agent status sidebar"
        >
          {/* Status */}
          <motion.div
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            className="flex items-center gap-2 rounded-sm bg-accent-subtle border border-accent/15 px-4 py-3"
            role="status"
            aria-live="polite"
          >
            <span
              className={`w-2 h-2 rounded-full ${statusColor(isDeploying(status) ? displayStatus : status, phase, unhealthy, gatewayAlive)} ${
                status === "active" && !unhealthy ? "animate-pulse" : ""
              }`}
            />
            <span className="text-[13px] font-semibold">
              {statusLabel(isDeploying(status) ? displayStatus : status, phase, unhealthy, gatewayAlive)}
            </span>
          </motion.div>

          <div>
            <div className="text-xs text-text-muted mb-1">Strategy</div>
            <div className="text-sm font-semibold">{data?.deployment?.strategyId ? formatStrategy(data.deployment.strategyId) : "—"}</div>
          </div>

          <div>
            <div className="text-xs text-text-muted mb-1">Uptime</div>
            <div className="font-mono text-sm">{isTerminated ? "—" : uptime}</div>
          </div>

          <div>
            <div className="text-xs text-text-muted mb-1">Hot Address</div>
            <div className="font-mono text-sm" title={data?.deployment?.hotAddress}>
              {data?.deployment?.hotAddress ? truncateAddress(data.deployment.hotAddress) : "—"}
            </div>
          </div>

          <div>
            <div className="text-xs text-text-muted mb-1">Hot Wallet Balance</div>
            <div className="font-mono text-sm font-semibold text-accent">
              {usdcBalance ? (
                <>${parseFloat(usdcBalance).toFixed(2)} <span className="text-text-muted font-normal">USDC</span></>
              ) : (
                <span className="text-text-muted font-normal">—</span>
              )}
            </div>
            {status === "active" && data?.deployment?.hotAddress && (
              <button
                onClick={() => setShowFunding(true)}
                className="mt-2 w-full py-1.5 text-center text-xs font-semibold text-accent border border-accent/20 rounded-sm hover:bg-accent/10 transition-colors"
              >
                Fund Agent
              </button>
            )}
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

          {status === "active" && (
            <div>
              <div className="text-xs text-text-muted mb-1">Backup</div>
              <div className="text-xs font-mono">
                {lastBackupAt ? (
                  <span className="text-success">
                    {Math.round((Date.now() - new Date(lastBackupAt).getTime()) / 60000)}min ago
                  </span>
                ) : (
                  <span className="text-warning">Pending...</span>
                )}
              </div>
            </div>
          )}

          {unhealthy && (
            <div className="text-xs text-warning" role="alert">
              Agent not responding. Last seen: {lastHeartbeat ? `${Math.round((Date.now() - new Date(lastHeartbeat).getTime()) / 1000)}s ago` : "never"}
            </div>
          )}
          {fetchError && !unhealthy && <div className="text-xs text-text-muted">Reconnecting...</div>}

          {/* Controls */}
          {isDeploying(status) ? (
            <button
              onClick={handleCancelDeploy}
              disabled={isTerminating}
              className="mt-auto py-3 text-center bg-border/50 text-text-muted border border-border rounded-sm font-semibold text-sm transition-all hover:bg-border hover:text-text min-h-[44px] disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Cancel Deploy
            </button>
          ) : isTerminated ? (
            <div className="mt-auto text-center text-sm text-text-muted py-3">Stopped</div>
          ) : (
            <div className="mt-auto flex flex-col gap-2">
              <button
                onClick={handlePauseResume}
                disabled={isTerminating || !agentReady}
                className={`py-2.5 text-center rounded-sm font-semibold text-sm transition-all min-h-[44px] disabled:opacity-50 disabled:cursor-not-allowed ${
                  isPaused
                    ? "bg-accent/10 text-accent border border-accent/20 hover:bg-accent/20"
                    : "bg-warning/10 text-warning border border-warning/20 hover:bg-warning/20"
                }`}
              >
                {isPaused ? "Resume Trading" : "Pause Trading"}
              </button>
              {showStopConfirm ? (
                <div className="flex gap-2">
                  <button
                    onClick={handleStopAgent}
                    className="flex-1 py-2 text-center bg-danger/20 text-danger rounded-sm text-xs font-semibold"
                  >
                    Confirm Stop
                  </button>
                  <button
                    onClick={() => setShowStopConfirm(false)}
                    className="flex-1 py-2 text-center bg-border/50 text-text-muted rounded-sm text-xs"
                  >
                    Cancel
                  </button>
                </div>
              ) : (
                <button
                  onClick={() => setShowStopConfirm(true)}
                  disabled={isTerminating}
                  className="py-2 text-center text-text-muted text-xs hover:text-danger transition-colors disabled:opacity-50"
                >
                  Stop Agent...
                </button>
              )}
            </div>
          )}

          {isTerminated && hasBackup && (
            <button
              onClick={handleRecover}
              disabled={isRecovering || isAuthenticating}
              className="py-3 text-center bg-accent/10 text-accent border border-accent/20 rounded-sm font-semibold text-sm transition-all hover:bg-accent/20 min-h-[44px] disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {isRecovering ? (isAuthenticating ? "Signing..." : "Recovering...") : "Recover Agent"}
            </button>
          )}
        </aside>

        {/* Main */}
        <main className="flex-1 p-4 sm:p-6 overflow-y-auto space-y-4" role="main" aria-label="Dashboard main content">
          {/* Deploy Progress */}
          {isDeploying(status) && (
            <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }}>
              <DeployProgress status={displayStatus} bidCount={bidCount} />
            </motion.div>
          )}

          {/* Failed Recovery */}
          {status === "failed" && (
            <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }}>
              <FailedRecovery error={sseError} deploymentId={deploymentId} locale={locale} />
            </motion.div>
          )}

          {/* P&L */}
          <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: 0.05 }} className="bg-surface rounded-md p-4 sm:p-6">
            <div className="text-[13px] text-text-muted mb-2">Total P&L</div>
            {isDeploying(status) ? (
              <div className="font-mono text-3xl sm:text-4xl font-semibold tabular-nums text-text-muted animate-pulse">—</div>
            ) : (
              <div className={`font-mono text-3xl sm:text-4xl font-semibold tabular-nums ${totalPnl >= 0 ? "text-success" : "text-danger"}`}>
                {totalPnl >= 0 ? "+" : ""}${totalPnl.toFixed(2)}
              </div>
            )}
          </motion.div>

          {/* Trade Log */}
          <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: 0.1 }} className="bg-surface rounded-md p-4">
            <div className="text-[13px] font-semibold text-text-muted mb-3">Recent Trades</div>
            {trades.length === 0 && (
              <div className="text-sm text-text-muted py-4 text-center">
                {isDeploying(status) ? "Waiting for agent to start..." : status === "active" ? "Agent is analyzing markets..." : "No trades yet."}
              </div>
            )}
            <AnimatePresence initial={false}>
              {trades.map((trade) => (
                <motion.div
                  key={trade.id}
                  initial={{ opacity: 0, x: -8 }}
                  animate={{ opacity: 1, x: 0, backgroundColor: newTradeIds.has(trade.id) ? "rgba(240, 160, 48, 0.15)" : "transparent" }}
                  exit={{ opacity: 0 }}
                  transition={{ backgroundColor: { duration: 2 } }}
                  className="grid grid-cols-[auto_1fr_auto_auto] gap-2 sm:gap-4 py-2.5 border-b border-border last:border-0 text-[13px] items-center rounded-sm"
                >
                  <span className="font-mono text-xs text-text-muted">{new Date(trade.ts).toLocaleTimeString("en-US", { hour12: false })}</span>
                  <span className="font-medium truncate">{trade.venue}</span>
                  <span className={`font-mono text-xs ${trade.action === "SELL" ? "text-danger" : "text-success"}`}>{trade.action}</span>
                  <span className={`font-mono font-semibold text-[13px] ${parseFloat(trade.pnlUsd) >= 0 ? "text-success" : "text-danger"}`}>
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
            <div className="text-[13px] font-semibold text-text-muted mb-3">Chat with Agent</div>
            <div className="min-h-[120px] max-h-[280px] overflow-y-auto mb-3 space-y-2 scrollbar-thin" role="log" aria-label="Chat messages" aria-live="polite">
              {allMessages.length === 0 ? (
                <div className="text-sm text-text-muted py-4 text-center">
                  {isTerminated ? "Agent terminated. Chat is read-only." : (isDeploying(status) || !agentReady) ? "Agent is starting up..." : "Say hi to your agent!"}
                </div>
              ) : (
                <AnimatePresence initial={false}>
                  {allMessages.map((msg) => {
                    const isUser = msg.direction === "user_to_agent";
                    return (
                      <motion.div key={msg.id} initial={{ opacity: 0, y: 4 }} animate={{ opacity: 1, y: 0 }} className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
                        <div className={`max-w-[85%] sm:max-w-[75%] px-3 py-2 rounded-md text-sm ${isUser ? "bg-accent/15 text-accent" : "bg-bg border border-border"}`}>
                          <div>{msg.content}</div>
                          <div className="text-[10px] text-text-muted mt-1 font-mono">
                            {new Date(msg.ts).toLocaleTimeString("en-US", { hour12: false })}
                          </div>
                        </div>
                      </motion.div>
                    );
                  })}
                </AnimatePresence>
              )}
              {isStreaming && streamingText && (
                <motion.div initial={{ opacity: 0, y: 4 }} animate={{ opacity: 1, y: 0 }} className="flex justify-start">
                  <div className="max-w-[85%] sm:max-w-[75%] px-3 py-2 rounded-md text-sm bg-bg border border-border">
                    <div>{streamingText}<span className="animate-pulse">▊</span></div>
                  </div>
                </motion.div>
              )}
              <div ref={chatEndRef} />
            </div>
            <div className="flex gap-2">
              <label htmlFor="chat-input" className="sr-only">Message to agent</label>
              <input
                id="chat-input"
                ref={inputRef}
                value={chatInput}
                onChange={(e) => setChatInput(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && handleSendChat()}
                placeholder={
                  isTerminated ? "Agent is terminated" : (isDeploying(status) || !agentReady) ? "Agent is starting up..." : "Ask your agent anything..."
                }
                maxLength={500}
                disabled={isTerminated || isDeploying(status) || !agentReady || isStreaming}
                className="flex-1 px-3.5 py-2.5 bg-bg border border-border rounded-sm text-sm outline-none focus:border-accent transition-colors min-h-[44px] disabled:opacity-50 disabled:cursor-not-allowed"
              />
              <button
                onClick={handleSendChat}
                disabled={isTerminated || isDeploying(status) || !agentReady || !chatInput.trim() || isStreaming}
                className="px-4 sm:px-5 py-2.5 bg-accent text-bg font-semibold text-sm rounded-sm hover:bg-accent-hover transition-colors min-h-[44px] disabled:opacity-50 disabled:cursor-not-allowed"
                aria-label="Send message"
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
