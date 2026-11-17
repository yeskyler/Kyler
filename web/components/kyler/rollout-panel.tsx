"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowDownIcon, ListTreeIcon, RefreshCwIcon, SearchIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api } from "@/lib/kyler/client";
import {
  formatCheckpointTokens,
  inspectionTime,
  mergeRollout,
  type RolloutDetail,
  type RolloutItem,
  type RolloutPage,
  type InspectionAnalytics,
} from "@/lib/kyler/inspection";

import { Sheet, SheetContent, SheetTitle, SheetDescription } from "@/components/ui/sheet";
import { InspectionMetrics } from "./inspection-ui";
import { RolloutCharts } from "./rollout-charts";

function Empty({ children }: { children: ReactNode }) {
  return (
    <p className="rounded-xl border border-dashed border-border/70 bg-muted/20 px-4 py-5 text-sm leading-6 text-muted-foreground">
      {children}
    </p>
  );
}
function ErrorNotice({ text }: { text: string }) {
  return text ? (
    <p
      role="alert"
      className="rounded-xl border border-destructive/15 bg-destructive/5 p-4 text-sm leading-6 text-destructive"
    >
      {text}
    </p>
  ) : null;
}
function RecordRow({ session, item }: { session: string; item: RolloutItem }) {
  const [data, setData] = useState<RolloutDetail>();
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [view, setView] = useState("original");
  const [open, setOpen] = useState(false);
  const operation = useRef<AbortController | null>(null);
  useEffect(() => () => operation.current?.abort(), []);
  async function load() {
    if (loading || data) return;
    const controller = new AbortController();
    operation.current = controller;
    setLoading(true);
    setError("");
    try {
      const value = await api<RolloutDetail>(`sessions/${session}/rollout/${item.seq}`, {
        signal: controller.signal,
      });
      if (!controller.signal.aborted) setData(value);
    } catch (error) {
      if (!controller.signal.aborted) setError((error as Error).message);
    } finally {
      if (!controller.signal.aborted) setLoading(false);
    }
  }
  const payload = data?.data.payload as
    | { event?: { content?: { type: string; content?: string; model_content?: string | null }[] } }
    | undefined;
  const outputs = payload?.event?.content?.filter((part) => part.type === "tool_result") ?? [];
  function downloadRaw() {
    if (!data) return;
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(data.data, null, 2)], { type: "application/json" }),
    );
    const link = document.createElement("a");
    link.href = url;
    link.download = `rollout-${item.seq}.json`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  }
  return (
    <>
      <button
        type="button"
        onClick={() => {
          setOpen(true);
          void load();
        }}
        className="flex w-full items-start gap-3 border-b border-border/50 py-4 text-left outline-none transition-colors last:border-0 hover:bg-muted/20 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50"
      >
        <span
          className={`mt-1 flex size-7 shrink-0 items-center justify-center rounded-lg ${item.status === "failed" ? "bg-destructive/10 text-destructive" : "bg-muted text-muted-foreground"}`}
        >
          <ListTreeIcon className="size-3.5" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block break-words text-xs font-medium">{item.title}</span>
          <span className="mt-1.5 flex flex-wrap items-center gap-2 text-[10px] text-muted-foreground">
            <span className="rounded bg-muted px-1.5 py-0.5 capitalize">{item.kind}</span>
            <span>#{item.seq}</span>
            <time>{new Date(item.timestamp_ms).toLocaleTimeString()}</time>
          </span>
        </span>
        {item.status && (
          <span
            className={`mt-1 shrink-0 rounded-md px-2 py-1 text-[10px] ${item.status === "failed" ? "bg-destructive/10 text-destructive" : "bg-muted text-muted-foreground"}`}
          >
            {item.status}
          </span>
        )}
      </button>
      <Sheet
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) {
            operation.current?.abort();
            setLoading(false);
          }
        }}
      >
        <SheetContent className="gap-0 data-[side=right]:w-full data-[side=right]:sm:max-w-2xl">
          <div className="shrink-0 border-b p-6 pr-12">
            <SheetTitle className="break-words text-lg">{item.title}</SheetTitle>
            <SheetDescription className="mt-2">
              {item.kind} · #{item.seq} · {inspectionTime(item.timestamp_ms)}
              {item.status ? ` · ${item.status}` : ""}
            </SheetDescription>
          </div>
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-6">
            <ErrorNotice text={error} />
            {loading && (
              <p role="status" className="text-xs text-muted-foreground">
                Loading event…
              </p>
            )}
            {error && (
              <Button variant="ghost" size="xs" onClick={load}>
                Retry
              </Button>
            )}
            {data && (
              <>
                {payload?.event?.content
                  ?.filter((part) => part.type === "message" || part.type === "reasoning")
                  .map((part, index) => (
                    <p
                      key={`message-${index}`}
                      className="whitespace-pre-wrap break-words text-sm leading-7"
                    >
                      {part.content || (part as { text?: string }).text}
                    </p>
                  ))}
                {outputs.length > 0 && (
                  <>
                    <div className="flex gap-1" role="group" aria-label="Tool output view">
                      {["original", "model"].map((value) => (
                        <button
                          key={value}
                          type="button"
                          aria-pressed={view === value}
                          onClick={() => setView(value)}
                          className={`rounded-md px-2 py-1 text-[11px] ${view === value ? "bg-muted" : "text-muted-foreground"}`}
                        >
                          {value === "original" ? "Original output" : "Model input"}
                        </button>
                      ))}
                    </div>
                    {outputs.map((output, index) => (
                      <pre
                        key={index}
                        className="max-h-80 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-muted/40 p-3 text-[11px] leading-5"
                      >
                        {view === "model"
                          ? (output.model_content ?? output.content)
                          : output.content}
                      </pre>
                    ))}
                  </>
                )}
                <details>
                  <summary className="cursor-pointer text-[11px] text-muted-foreground">
                    Checkpoint details · k tokens
                  </summary>
                  <pre className="mt-2 max-h-96 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-muted/40 p-3 text-[10px] leading-5">
                    {JSON.stringify(formatCheckpointTokens(data.data), null, 2)}
                  </pre>
                </details>
                <Button variant="ghost" size="xs" onClick={downloadRaw}>
                  Download event JSON
                </Button>
              </>
            )}
          </div>
        </SheetContent>
      </Sheet>
    </>
  );
}

export function RolloutPanel({
  session,
  active = true,
  analytics,
}: {
  session: string;
  active?: boolean;
  analytics?: InspectionAnalytics;
}) {
  const [items, setItems] = useState<RolloutItem[]>([]);
  const [more, setMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState("activity");
  const [query, setQuery] = useState("");
  const [follow, setFollow] = useState(false);
  const cursor = useRef(0);
  const fetching = useRef(false);
  const hasMore = useRef(false);
  const operation = useRef<AbortController | null>(null);
  const log = useRef<HTMLDivElement>(null);
  const scroller = useRef<HTMLElement | null>(null);
  useEffect(() => {
    if (!active) return;
    const element = log.current?.closest<HTMLElement>("[data-inspector-scroll]");
    if (!element) return;
    scroller.current = element;
    const update = () =>
      setFollow(element.scrollHeight - element.scrollTop - element.clientHeight < 32);
    element.addEventListener("scroll", update, { passive: true });
    return () => element.removeEventListener("scroll", update);
  }, [active]);
  const load = useCallback(async () => {
    if (fetching.current) return;
    fetching.current = true;
    setBusy(true);
    const controller = new AbortController();
    operation.current = controller;
    try {
      const page = await api<RolloutPage>(
        `sessions/${session}/rollout?cursor=${cursor.current}&limit=60`,
        { signal: controller.signal },
      );
      if (page.session_id !== session) throw new Error("Rollout session mismatch.");
      if (!controller.signal.aborted) {
        setItems((items) => mergeRollout(items, page.items));
        cursor.current = page.cursor;
        hasMore.current = page.next_cursor != null;
        setMore(hasMore.current);
        setError("");
      }
    } catch (error) {
      if (!controller.signal.aborted) setError((error as Error).message);
    } finally {
      fetching.current = false;
      if (!controller.signal.aborted) setBusy(false);
    }
  }, [session]);
  useEffect(() => {
    if (!active) return;
    void load();
    const timer = setInterval(() => {
      if (!hasMore.current && document.visibilityState !== "hidden") void load();
    }, 3000);
    return () => {
      clearInterval(timer);
      operation.current?.abort();
    };
  }, [load, active]);
  useEffect(() => {
    const element = scroller.current;
    if (active && follow && element) element.scrollTop = element.scrollHeight;
  }, [items.length, follow, filter, active]);
  const visible = items
    .filter((item) =>
      filter === "all"
        ? true
        : filter === "errors"
          ? item.status === "failed"
          : filter === "activity"
            ? !["state", "usage", "checkpoint", "session", "environment"].includes(item.kind) &&
              item.title !== "Context checkpoint"
            : item.kind === filter,
    )
    .filter((item) =>
      `${item.title} ${item.kind} ${item.execution_id ?? ""} ${item.seq}`
        .toLowerCase()
        .includes(query.trim().toLowerCase()),
    );
  return (
    <div className="space-y-6">
      <ErrorNotice text={error} />
      <InspectionMetrics
        items={[
          {
            label: "Executions",
            value: analytics?.execution_count ?? "—",
            hint: "Complete committed session",
          },
          {
            label: "Tool calls",
            value: analytics?.tool_calls ?? "—",
            hint: "Unique committed calls",
          },
          {
            label: "Failed tool calls",
            value: analytics?.failed_tool_calls ?? "—",
            hint: "Recorded error results",
          },
          {
            label: "Last activity",
            value: analytics?.last_activity_ms
              ? new Date(analytics.last_activity_ms).toLocaleTimeString([], {
                  hour: "2-digit",
                  minute: "2-digit",
                })
              : "—",
            hint: analytics
              ? `${analytics.records.toLocaleString()} committed records`
              : "Chart history not loaded",
          },
        ]}
      />
      <RolloutCharts
        analytics={analytics}
        active={active}
        onTool={(name) => {
          setQuery(name);
          setFilter("tool");
        }}
      />
      <div className="flex flex-wrap items-center gap-3 rounded-xl border border-border/65 bg-card/50 p-3">
        <div className="relative min-w-0 flex-1 basis-48">
          <SearchIcon
            aria-hidden
            className="absolute left-2.5 top-2.5 size-3.5 text-muted-foreground"
          />
          <Input
            aria-label="Search rollout events"
            placeholder="Find a tool, event, or execution"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="h-9 bg-background/60 pl-8 text-xs"
          />
        </div>
        <select
          aria-label="Filter rollout events"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          className="h-9 min-w-0 rounded-lg border border-border/70 bg-background/60 px-3 text-xs"
        >
          {[
            ["activity", "Activity"],
            ["all", "All records"],
            ["message", "Messages"],
            ["tool", "Tools"],
            ["errors", "Errors"],
            ["context", "Context"],
            ["execution", "Executions"],
            ["subagent", "Subagents"],
          ].map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
        <Button
          type="button"
          size="sm"
          variant={follow ? "secondary" : "outline"}
          aria-pressed={follow}
          disabled={more}
          title={
            more
              ? "Load remaining events before following the latest."
              : "Follow the latest committed events"
          }
          onClick={() => setFollow(!follow)}
        >
          <ArrowDownIcon className="size-3.5" />
          {follow ? "Following" : "Follow latest"}
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Refresh rollout"
          title="Refresh rollout"
          disabled={busy}
          onClick={() => void load()}
        >
          <RefreshCwIcon className={`size-3.5 ${busy ? "animate-spin" : ""}`} />
        </Button>
      </div>
      <div
        ref={log}
        className="rounded-2xl border border-border/70 bg-card/60 p-5 sm:p-6"
        aria-label="Committed rollout timeline"
      >
        <div className="mb-5 flex items-center justify-between gap-3">
          <h2 className="text-sm font-semibold">Execution timeline</h2>
          <span className="text-[11px] text-muted-foreground">
            {visible.length} of {items.length} loaded records
          </span>
        </div>
        {!visible.length ? (
          <Empty>
            {busy
              ? "Loading rollout…"
              : query || filter !== "activity"
                ? "No matching loaded events. Try another filter or load more records."
                : "No execution events recorded yet."}
          </Empty>
        ) : (
          visible.map((item, index) => (
            <div key={item.seq}>
              {item.execution_id &&
                item.execution_id !== visible[index - 1]?.execution_id &&
                (() => {
                  const execution = analytics?.executions.find(
                    (entry) => entry.execution_id === item.execution_id,
                  );
                  return (
                    <div className="mb-1 mt-6 flex flex-wrap items-center justify-between gap-2 border-b pb-3 text-xs">
                      <h3 title={item.execution_id} className="font-medium">
                        Execution {item.execution_id.slice(0, 8)}
                      </h3>
                      <span className="text-[10px] text-muted-foreground">
                        {execution?.status}
                        {execution?.finished_at != null
                          ? ` · ${((execution.finished_at - execution.started_at) / 1000).toFixed(1)}s`
                          : ""}
                        {execution ? ` · ${execution.steps} steps` : ""}
                      </span>
                    </div>
                  );
                })()}
              <RecordRow session={session} item={item} />
            </div>
          ))
        )}
      </div>
      {more && (
        <Button variant="outline" size="sm" disabled={busy} onClick={() => void load()}>
          Load more events
        </Button>
      )}
      <p className="text-[10px] leading-5 text-muted-foreground">
        Committed checkpoints only · List refreshes every 3s while open. Charts summarize the
        complete session and refresh on opening, turn completion, focus or manual refresh. Sensitive
        argument keys are redacted; large strings are shown as bounded previews.
      </p>
    </div>
  );
}
