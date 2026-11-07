"use client";

import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useAuiState } from "@assistant-ui/react";
import { ArrowLeftIcon, BrainIcon, ChartPieIcon, ListTreeIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/kyler/client";
import {
  contextUsage,
  tokenCount,
  type ContextInspection,
  type InspectorTab,
} from "@/lib/kyler/inspection";
import { ContextPanel, MemoryPanel, RolloutPanel } from "./inspection-panels";

const tabs = [
  { id: "rollout", label: "Rollout", Icon: ListTreeIcon },
  { id: "memory", label: "Memory", Icon: BrainIcon },
  { id: "context", label: "Context", Icon: ChartPieIcon },
] as const;
type InspectionState = {
  session: string;
  tab: InspectorTab | null;
  setTab: (tab: InspectorTab | null) => void;
  context?: ContextInspection;
  error: string;
  loading: boolean;
  active: boolean;
  visited: Partial<Record<InspectorTab, boolean>>;
  navigate: (tab: InspectorTab, target: HTMLButtonElement) => void;
  back: () => void;
};
const Inspection = createContext<InspectionState | null>(null);
function useInspection() {
  const value = useContext(Inspection);
  if (!value) throw new Error("Inspector requires a session.");
  return value;
}

export function SessionInspectorProvider({
  session,
  active,
  enabled,
  onOpen,
  onBack,
  children,
}: {
  session: string;
  active: boolean;
  enabled: boolean;
  onOpen: () => void;
  onBack: () => void;
  children: ReactNode;
}) {
  const [tab, setTabState] = useState<InspectorTab | null>(null);
  const [visited, setVisited] = useState<Partial<Record<InspectorTab, boolean>>>({});
  const setTab = (value: InspectorTab | null) => {
    if (value) setVisited((previous) => ({ ...previous, [value]: true }));
    setTabState(value);
  };
  const [context, setContext] = useState<ContextInspection>();
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const opener = useRef<HTMLButtonElement | null>(null);
  const running = useAuiState((state) => state.thread.isRunning);
  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    let busy = false;
    async function refresh() {
      if (busy || document.visibilityState === "hidden") return;
      busy = true;
      try {
        const value = await api<ContextInspection>(`sessions/${session}/context`, {
          signal: controller.signal,
        });
        if (value.session_id !== session) throw new Error("Session context mismatch.");
        if (!controller.signal.aborted) {
          setContext(value);
          setError("");
        }
      } catch (error) {
        if (!controller.signal.aborted) setError((error as Error).message);
      } finally {
        busy = false;
        if (!controller.signal.aborted) setLoading(false);
      }
    }
    void refresh();
    const timer = running || active ? setInterval(refresh, 3000) : undefined;
    document.addEventListener("visibilitychange", refresh);
    return () => {
      controller.abort();
      clearInterval(timer);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [session, running, active, enabled]);
  return (
    <Inspection.Provider
      value={{
        session,
        tab,
        setTab,
        context,
        error,
        loading,
        active,
        visited,
        navigate: (next, target) => {
          if (!enabled) return;
          opener.current = target;
          setTab(next);
          onOpen();
        },
        back: () => {
          onBack();
          requestAnimationFrame(() => opener.current?.isConnected && opener.current.focus());
        },
      }}
    >
      {children}
    </Inspection.Provider>
  );
}

export function SessionInspectorPage() {
  const { session, tab, setTab, active, visited, context, error, loading } = useInspection();
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (active) heading.current?.focus();
  }, [active]);
  const selected = tabs.find((item) => item.id === tab);
  return (
    <section
      id="session-inspector-page"
      className="h-full min-h-0 overflow-y-auto"
      data-inspector-scroll
      aria-label="Session inspector"
    >
      <header className="px-6 pb-0 pt-6 sm:px-10">
        <div className="mx-auto w-full max-w-5xl">
          <h1
            ref={heading}
            tabIndex={-1}
            className="text-2xl font-semibold tracking-tight outline-none"
          >
            {selected?.label ?? "Session inspector"}
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            {tab === "rollout"
              ? "Execution history and committed checkpoints."
              : tab === "memory"
                ? "Session summary and saved workspace memory."
                : "Model input, context budget, and compaction."}
          </p>
          <p title={session} className="mt-2 break-all font-mono text-[11px] text-muted-foreground">
            Session {session}
          </p>
          <div role="tablist" aria-label="Session inspection" className="mt-5 flex gap-1">
            {tabs.map(({ id, label, Icon }) => (
              <button
                key={id}
                id={`inspect-tab-${id}`}
                role="tab"
                type="button"
                aria-selected={tab === id}
                aria-controls={`inspect-panel-${id}`}
                tabIndex={tab === id ? 0 : -1}
                onClick={() => setTab(id)}
                onKeyDown={(event) => {
                  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
                  event.preventDefault();
                  const index = tabs.findIndex((item) => item.id === tab);
                  const next =
                    event.key === "Home"
                      ? 0
                      : event.key === "End"
                        ? 2
                        : (index + (event.key === "ArrowRight" ? 1 : 2)) % 3;
                  setTab(tabs[next].id);
                  document.getElementById(`inspect-tab-${tabs[next].id}`)?.focus();
                }}
                className={`flex items-center gap-2 rounded-lg px-3 py-2 text-sm sm:px-4 ${tab === id ? "bg-foreground font-medium text-background" : "text-muted-foreground hover:bg-muted"}`}
              >
                <Icon className="size-4" />
                {label}
              </button>
            ))}
          </div>
        </div>
      </header>
      <div>
        {tabs.map(({ id }) => (
          <div
            key={id}
            hidden={tab !== id}
            className="px-6 py-6 sm:px-10"
            id={`inspect-panel-${id}`}
            role="tabpanel"
            aria-labelledby={`inspect-tab-${id}`}
            tabIndex={0}
          >
            <div className="mx-auto w-full max-w-5xl">
              {visited[id] && id === "rollout" && (
                <RolloutPanel session={session} active={active && tab === id} />
              )}
              {visited[id] && id === "memory" && (
                <MemoryPanel session={session} active={active && tab === id} />
              )}
              {visited[id] && id === "context" && (
                <ContextPanel data={context} error={error} loading={loading} />
              )}
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

export function SessionInspectorBackButton() {
  const { active, back } = useInspection();
  if (!active) return null;
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      className="-ml-1 text-muted-foreground"
      onClick={back}
    >
      <ArrowLeftIcon className="size-3.5" />
      Back to chat
    </Button>
  );
}

export function SessionInspectorControls() {
  const { tab, active, navigate, context, error } = useInspection();
  const usage = contextUsage(context?.latest_request);
  return (
    <div
      className="ml-auto flex shrink-0 items-center gap-0.5"
      aria-label="Session inspection tools"
    >
      {tabs.map(({ id, label, Icon }) => {
        const title =
          id === "context"
            ? error
              ? "Context unavailable"
              : usage
                ? `${label} · ${tokenCount(usage.tokens)} / ${tokenCount(usage.capacity)} tokens${usage.estimated ? " (estimated)" : " (provider reported)"}`
                : "Context · No captured request"
            : label;
        return (
          <button
            key={id}
            type="button"
            aria-label={title}
            title={title}
            aria-controls="session-inspector-page"
            aria-current={active && tab === id ? "page" : undefined}
            onClick={(event) => navigate(id, event.currentTarget)}
            className={`flex h-6 items-center justify-center gap-1 rounded-md px-1.5 outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 ${active && tab === id ? "bg-muted text-foreground" : "text-muted-foreground"}`}
          >
            <Icon aria-hidden className="size-3.5" strokeWidth={1.5} />
            {id === "context" && usage && (
              <span className="text-[10px] tabular-nums">{usage.percent}%</span>
            )}
          </button>
        );
      })}
    </div>
  );
}
