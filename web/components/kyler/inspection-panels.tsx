"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  ArrowDownIcon,
  ArrowRightIcon,
  ChevronDownIcon,
  RefreshCwIcon,
  SearchIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api } from "@/lib/kyler/client";
import {
  contextUsage,
  contextComponents,
  formatCheckpointTokens,
  inspectionTime,
  mergeRollout,
  tokenCount,
  type ContextInspection,
  type MemoryInspection,
  type RolloutDetail,
  type RolloutItem,
  type RolloutPage,
} from "@/lib/kyler/inspection";

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
function Section({
  title,
  description,
  children,
  className = "",
}: {
  title: string;
  description?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={`min-w-0 space-y-4 rounded-2xl border border-border/70 bg-card/60 p-5 sm:p-6 ${className}`}
    >
      <div>
        <h2 className="text-sm font-semibold tracking-tight">{title}</h2>
        {description && (
          <p className="mt-1 text-xs leading-5 text-muted-foreground">{description}</p>
        )}
      </div>
      {children}
    </section>
  );
}
function Values({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="text-xs sm:text-[13px]">
      {rows.map(([key, value]) => (
        <div
          key={key}
          className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.35fr)] items-start gap-5 border-b border-border/35 py-2.5 first:pt-0 last:border-0 last:pb-0"
        >
          <dt className="leading-5 text-muted-foreground">{key}</dt>
          <dd className="text-right font-medium leading-5 tabular-nums [overflow-wrap:anywhere]">
            {value}
          </dd>
        </div>
      ))}
    </dl>
  );
}
function Metric({ label, value, hint }: { label: string; value: ReactNode; hint?: string }) {
  return (
    <div className="min-w-0">
      <dt className="text-[11px] leading-5 text-muted-foreground">{label}</dt>
      <dd className="mt-1 break-words text-xl font-medium tracking-tight tabular-nums sm:text-2xl">
        {value}
      </dd>
      {hint && <p className="mt-1 text-[11px] leading-5 text-muted-foreground">{hint}</p>}
    </div>
  );
}

export function ContextPanel({
  data,
  loading,
  error,
}: {
  data?: ContextInspection;
  loading: boolean;
  error: string;
}) {
  if (!data)
    return (
      <>
        <ErrorNotice text={error} />
        {loading ? (
          <p role="status" className="text-xs text-muted-foreground">
            Loading context…
          </p>
        ) : (
          <Empty>Context is unavailable.</Empty>
        )}
      </>
    );
  const request = data.latest_request;
  const usage = contextUsage(request);
  const components = request ? contextComponents(request) : [];
  const budget = request?.budget ?? data.next_request_budget;
  const next = data.next_request_budget;
  const palette = ["bg-foreground/75", "bg-foreground/55", "bg-foreground/35", "bg-foreground/20"];
  return (
    <div className="flex flex-col gap-6">
      <ErrorNotice text={error || data.budget_error || ""} />
      <Section title="Context overview" description="Latest prepared model request">
        {usage && request ? (
          <>
            <div className="flex flex-wrap items-end justify-between gap-5">
              <div className="flex items-baseline gap-2">
                <strong className="text-5xl font-medium tracking-tight tabular-nums">
                  {usage.percent}
                  <span className="ml-0.5 text-2xl text-muted-foreground">%</span>
                </strong>
                <span className="text-xs text-muted-foreground">window utilization</span>
              </div>
              <div className="min-w-0 text-left sm:text-right">
                <span className="inline-flex rounded-full bg-muted px-2.5 py-1 text-[10px] font-medium text-muted-foreground">
                  {usage.estimated ? "Estimated input" : "Provider-reported input"}
                </span>
                <p className="mt-2 break-all text-sm font-medium">{request.budget.model}</p>
                <p className="mt-1 text-[11px] text-muted-foreground">
                  Step {request.step} · {inspectionTime(request.timestamp_ms)}
                </p>
              </div>
            </div>
            <div
              role="progressbar"
              aria-label="Context input usage"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.min(100, Math.max(0, usage.percent))}
              aria-valuetext={`${usage.estimated ? "Estimated " : ""}${tokenCount(usage.tokens)} of ${tokenCount(usage.capacity)} tokens`}
              className="h-2.5 overflow-hidden rounded-full bg-muted"
            >
              <div
                className={`h-full rounded-full ${usage.percent >= 80 ? "bg-amber-500/80" : "bg-foreground/65"}`}
                style={{ width: `${Math.min(100, Math.max(0, usage.percent))}%` }}
              />
            </div>
            <dl className="grid gap-5 border-t border-border/50 pt-4 sm:grid-cols-3">
              <Metric
                label="Input tokens"
                value={`${usage.estimated ? "~" : ""}${tokenCount(usage.tokens)}`}
              />
              <Metric label="Window capacity" value={tokenCount(usage.capacity)} />
              <Metric
                label="Available after reservations"
                value={tokenCount(
                  Math.max(
                    0,
                    usage.capacity -
                      usage.tokens -
                      request.budget.output_tokens -
                      request.budget.safety_tokens,
                  ),
                )}
              />
            </dl>
            <p className="text-[11px] leading-5 text-muted-foreground">
              Captured before the request. Its model budget stays fixed when model settings change.
            </p>
          </>
        ) : (
          <Empty>
            No captured request yet. New turns will populate this view; older runs may not contain a
            snapshot.
          </Empty>
        )}
      </Section>
      {request && components.length > 0 && (
        <Section
          title="Input composition"
          description="Estimated breakdown of the input shown above"
        >
          <div className="flex h-2 overflow-hidden rounded-full bg-muted" aria-hidden>
            {components.map((component, index) => (
              <div
                key={component.name}
                className={palette[index % palette.length]}
                style={{
                  width: `${usage?.tokens ? (component.estimated_tokens / usage.tokens) * 100 : 0}%`,
                }}
              />
            ))}
          </div>
          <Values
            rows={components.map((component, index) => [
              component.name === "system"
                ? "System & environment"
                : component.name === "tools"
                  ? "Tool definitions"
                  : component.name === "memory"
                    ? "Memory & summary"
                    : "Conversation",
              <span key={component.name} className="inline-flex items-center gap-2">
                <span className={`size-2 rounded-sm ${palette[index % palette.length]}`} />~
                {tokenCount(component.estimated_tokens)}
              </span>,
            ])}
          />
          <p className="text-[11px] leading-5 text-muted-foreground">
            Providers report only the total. Categories are allocated proportionally to that total
            when available, or to the local estimate, including framing and image reservations.
          </p>
        </Section>
      )}
      {budget && (
        <Section title="Context policy">
          <Values
            rows={[
              ["Reserved output", tokenCount(budget.output_tokens)],
              ["Safety margin", tokenCount(budget.safety_tokens)],
              ["Input / compaction limit", tokenCount(budget.input_limit)],
              ["Sliding window target", tokenCount(budget.sliding_window_tokens)],
            ]}
          />
        </Section>
      )}
      {next &&
        request &&
        (next.model !== request.budget.model ||
          next.context_tokens !== request.budget.context_tokens) && (
          <Section title="Next request">
            <Values
              rows={[
                ["Selected model", next.model],
                ["Window capacity", tokenCount(next.context_tokens)],
              ]}
            />
            <p className="text-[11px] text-muted-foreground">
              Applied to the next admitted execution.
            </p>
          </Section>
        )}
      <Section title="Compaction">
        <Values
          rows={[
            ["Compressions", String(data.compression_count)],
            ["Window starts at event", String(data.window_start)],
            ["Total recorded events", String(data.total_events)],
          ]}
        />
        {data.last_compaction && (
          <div className="rounded-xl bg-muted/35 p-4">
            <dl className="grid grid-cols-[1fr_auto_1fr] items-center gap-4">
              <Metric
                label="Before (estimated)"
                value={`~${tokenCount(data.last_compaction.before_tokens)}`}
              />
              <ArrowRightIcon aria-hidden className="size-4 text-muted-foreground" />
              <Metric
                label="After (estimated)"
                value={`~${tokenCount(data.last_compaction.after_tokens)}`}
              />
            </dl>
            <p className="mt-3 text-[11px] text-muted-foreground">
              {inspectionTime(data.last_compaction.timestamp_ms)}
            </p>
          </div>
        )}
      </Section>
      <Section
        title="Cumulative session usage"
        description="Reported requests, including subagents; separate from context occupancy"
      >
        <dl className="grid grid-cols-2 gap-5">
          <Metric label="Input tokens" value={tokenCount(data.cumulative_input_tokens)} />
          <Metric label="Output tokens" value={tokenCount(data.cumulative_output_tokens)} />
        </dl>
        <p className="text-[11px] leading-5 text-muted-foreground">
          Provider-reported usage across the session and delegated work. Requests without usage
          reports are not included; these totals are not context occupancy.
        </p>
      </Section>
    </div>
  );
}

export function MemoryPanel({ session, active = true }: { session: string; active?: boolean }) {
  const [data, setData] = useState<MemoryInspection>();
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    let busy = false;
    async function refresh() {
      if (busy || document.visibilityState === "hidden") return;
      busy = true;
      try {
        const value = await api<MemoryInspection>(`sessions/${session}/memory/inspect`, {
          signal: controller.signal,
        });
        if (value.session_id !== session) throw new Error("Session memory mismatch.");
        if (!controller.signal.aborted) {
          setData(value);
          setError("");
        }
      } catch (error) {
        if (!controller.signal.aborted) setError((error as Error).message);
      } finally {
        busy = false;
      }
    }
    void refresh();
    const timer = setInterval(refresh, 5000);
    return () => {
      controller.abort();
      clearInterval(timer);
    };
  }, [session, active]);
  const entries =
    data?.entries.filter((entry) =>
      `${entry.key} ${entry.content} ${entry.tags.join(" ")}`
        .toLowerCase()
        .includes(query.trim().toLowerCase()),
    ) ?? [];
  const operationRow = (entry: MemoryInspection["activity"][number]) => (
    <li
      key={`${entry.execution_id}:${entry.call_id}`}
      className="flex items-start justify-between gap-3 border-b border-border/40 py-2.5 first:pt-0 last:border-0 last:pb-0"
    >
      <div className="min-w-0">
        <p className="break-words text-xs font-medium">
          {entry.action || "Memory operation"}
          {entry.key ? ` · ${entry.key}` : ""}
        </p>
        <time className="mt-1 block text-[10px] text-muted-foreground">
          {inspectionTime(entry.timestamp * 1000)}
        </time>
      </div>
      <span
        className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] ${entry.status === "error" ? "bg-destructive/5 text-destructive" : "bg-muted text-muted-foreground"}`}
      >
        {entry.status || "pending"}
      </span>
    </li>
  );
  return (
    <div className="flex flex-col gap-6">
      <ErrorNotice text={error} />
      {!data ? (
        <p role="status" className="text-sm text-muted-foreground">
          Loading memory…
        </p>
      ) : (
        <>
          <Section
            title="Memory overview"
            description="Session context and explicit long-term facts"
          >
            <dl className="grid gap-5 sm:grid-cols-3">
              <Metric label="Saved facts" value={data.entries.length} />
              <Metric label="Used in latest request" value={data.retrieved_keys.length} />
              <Metric label="Context compressions" value={data.compression_count} />
            </dl>
            <p className="break-words border-t border-border/50 pt-4 text-[11px] leading-5 text-muted-foreground">
              {data.workspace_root
                ? `Shared workspace · ${data.workspace_root}`
                : "Persistent memory is scoped to this session."}
            </p>
          </Section>
          <Section
            title="Session summary"
            description="Compressed history used by context management"
          >
            {data.summary ? (
              <>
                <p className="whitespace-pre-wrap break-words text-sm leading-7">{data.summary}</p>
                <div className="border-t border-border/50 pt-4">
                  <Values
                    rows={[
                      ["Updated", inspectionTime(data.summary_timestamp_ms)],
                      ["Window starts at event", String(data.summarized_events)],
                    ]}
                  />
                </div>
              </>
            ) : (
              <Empty>
                No compressed summary yet. It appears when the context window needs compression.
              </Empty>
            )}
          </Section>
          <Section
            title="Memory activity"
            description="Recent explicit tool calls, including failed attempts"
          >
            {!data.activity.length ? (
              <Empty>No memory operations recorded in this session.</Empty>
            ) : (
              <>
                <ol>{data.activity.slice(0, 6).map(operationRow)}</ol>
                {data.activity.length > 6 && (
                  <details>
                    <summary className="cursor-pointer text-xs text-muted-foreground">
                      {data.activity.length - 6} more operations
                    </summary>
                    <ol className="mt-3">{data.activity.slice(6).map(operationRow)}</ol>
                  </details>
                )}
              </>
            )}
          </Section>
          <Section
            title="Persistent memory"
            description="Saved facts and preferences. Chat history is not automatically stored here."
          >
            <div className="flex flex-wrap items-center justify-between gap-3">
              <span className="text-xs text-muted-foreground">
                {query
                  ? `${entries.length} of ${data.entries.length} facts`
                  : `${data.entries.length} saved facts`}
              </span>
              <div className="relative w-full sm:max-w-xs">
                <SearchIcon
                  aria-hidden
                  className="absolute left-2.5 top-2.5 size-3.5 text-muted-foreground"
                />
                <Input
                  aria-label="Search saved memory"
                  placeholder="Search facts, tags, or content"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  className="h-9 pl-8 text-xs"
                />
              </div>
            </div>
            {!entries.length ? (
              <Empty>
                {query
                  ? "No matching facts. Try another search."
                  : "No saved long-term memory yet."}
              </Empty>
            ) : (
              <div className="grid items-start gap-3">
                {entries.map((entry) => (
                  <details
                    key={entry.key}
                    className="group/fact min-w-0 rounded-xl border border-border/65 bg-background/50 p-4"
                  >
                    <summary className="flex cursor-pointer list-none items-start justify-between gap-3 outline-none focus-visible:ring-2 focus-visible:ring-ring/50">
                      <span className="min-w-0">
                        <span className="block break-words text-sm font-medium">{entry.key}</span>
                        {data.retrieved_keys.includes(entry.key) && (
                          <span className="mt-1.5 inline-flex rounded-full bg-muted px-2 py-0.5 text-[10px] text-muted-foreground">
                            Used in latest request
                          </span>
                        )}
                        <span className="mt-2 line-clamp-2 text-xs leading-6 text-muted-foreground group-open/fact:hidden">
                          {entry.content}
                        </span>
                      </span>
                      <ChevronDownIcon
                        aria-hidden
                        className="mt-0.5 size-3.5 shrink-0 text-muted-foreground transition-transform group-open/fact:rotate-180"
                      />
                    </summary>
                    <div className="mt-4 space-y-4 border-t border-border/50 pt-4">
                      <p className="whitespace-pre-wrap break-words text-sm leading-7">
                        {entry.content}
                      </p>
                      <Values
                        rows={[
                          ["Revision", String(entry.revision)],
                          ["Updated", inspectionTime(entry.updated_at)],
                          [
                            "Source session",
                            <span key="source" className="font-mono text-[10px]">
                              {entry.source_session}
                            </span>,
                          ],
                          ["Tags", entry.tags.join(", ") || "None"],
                        ]}
                      />
                    </div>
                  </details>
                ))}
              </div>
            )}
          </Section>
        </>
      )}
    </div>
  );
}

function RecordRow({ session, item }: { session: string; item: RolloutItem }) {
  const [data, setData] = useState<RolloutDetail>();
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [view, setView] = useState("original");
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
    <details
      className="group/record border-l border-border/65 pb-6 pl-6 last:pb-1"
      onToggle={(event) => {
        if (event.currentTarget.open) void load();
      }}
    >
      <summary className="relative cursor-pointer list-none rounded-md text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50">
        <span
          aria-hidden
          className={`absolute -left-[28px] top-1.5 size-2 rounded-full ring-4 ring-card ${item.status === "failed" ? "bg-destructive" : "bg-muted-foreground/40"}`}
        />
        <span className="flex items-start justify-between gap-2">
          <span className="min-w-0 break-words font-medium">{item.title}</span>
          <ChevronDownIcon className="size-3 shrink-0 text-muted-foreground transition-transform group-open/record:rotate-180" />
        </span>
        <span className="mt-1.5 flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
          <span className="rounded-md bg-muted/70 px-1.5 py-0.5 text-[10px] capitalize">
            {item.kind}
          </span>
          <span className="font-mono text-[10px]">#{item.seq}</span>
          <span>{new Date(item.timestamp_ms).toLocaleTimeString()}</span>
          {item.status && (
            <span className={item.status === "failed" ? "text-destructive" : ""}>
              {item.status}
            </span>
          )}
        </span>
      </summary>
      <div className="mt-4 space-y-3 rounded-xl border border-border/50 bg-background/60 p-4">
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
                    {view === "model" ? (output.model_content ?? output.content) : output.content}
                  </pre>
                ))}
              </>
            )}
            <details open={!outputs.length}>
              <summary className="cursor-pointer text-[11px] text-muted-foreground">
                Checkpoint details · k tokens
              </summary>
              <pre className="mt-2 max-h-96 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-muted/40 p-3 text-[10px] leading-5">
                {JSON.stringify(formatCheckpointTokens(data.data), null, 2)}
              </pre>
            </details>
            <Button variant="ghost" size="xs" onClick={downloadRaw}>
              Download raw JSON
            </Button>
          </>
        )}
      </div>
    </details>
  );
}

export function RolloutPanel({ session, active = true }: { session: string; active?: boolean }) {
  const [items, setItems] = useState<RolloutItem[]>([]);
  const [more, setMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState("activity");
  const [query, setQuery] = useState("");
  const [follow, setFollow] = useState(true);
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
      <section
        aria-label="Loaded rollout overview"
        className="rounded-2xl border border-border/70 bg-card/60 p-5 sm:p-6"
      >
        <dl className="grid gap-5 sm:grid-cols-3">
          <Metric label="Records loaded" value={items.length} />
          <Metric
            label="Executions in loaded records"
            value={new Set(items.map((item) => item.execution_id).filter(Boolean)).size}
          />
          <Metric
            label="Failure records"
            value={items.filter((item) => item.status === "failed").length}
          />
        </dl>
      </section>
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
              {item.execution_id && item.execution_id !== visible[index - 1]?.execution_id && (
                <h3
                  title={item.execution_id}
                  className="mb-4 mt-6 border-t border-border/50 pt-5 font-mono text-[11px] text-muted-foreground"
                >
                  Execution {item.execution_id.slice(0, 8)}
                </h3>
              )}
              <RecordRow session={session} item={item} />
            </div>
          ))
        )}
      </div>
      {!follow && (
        <Button variant="ghost" size="xs" onClick={() => setFollow(true)}>
          <ArrowDownIcon className="size-3" />
          Follow latest
        </Button>
      )}
      {more && (
        <Button variant="outline" size="sm" disabled={busy} onClick={() => void load()}>
          Load more events
        </Button>
      )}
      <p className="text-[10px] leading-5 text-muted-foreground">
        Committed checkpoints only · Refreshes every 3s while open. Uncommitted writes are excluded.
        Sensitive argument keys are redacted; large strings are shown as bounded previews.
      </p>
    </div>
  );
}
