"use client";

import { useEffect, useState } from "react";
import { Bar, BarChart, CartesianGrid, Tooltip, XAxis, YAxis } from "recharts";
import { BrainIcon, SearchIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Sheet, SheetContent, SheetTitle, SheetDescription } from "@/components/ui/sheet";
import { api } from "@/lib/kyler/client";
import {
  inspectionTime,
  memoryActivityBuckets,
  memoryTagCounts,
  type MemoryInspection,
} from "@/lib/kyler/inspection";
import {
  ChartFrame,
  ChartKey,
  ChartTooltip,
  InspectionCard,
  InspectionEmpty,
  InspectionError,
  InspectionMetrics,
  InspectionSkeleton,
  axisStyle,
  chartColors,
} from "./inspection-ui";

export function MemoryPanel({ session, active = true }: { session: string; active?: boolean }) {
  const [data, setData] = useState<MemoryInspection>();
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState("");
  const [usedOnly, setUsedOnly] = useState(false);
  const [selectedKey, setSelectedKey] = useState<string>();
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
      } catch (failure) {
        if (!controller.signal.aborted) setError((failure as Error).message);
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
  if (!data)
    return (
      <div className="space-y-5">
        <InspectionError>{error}</InspectionError>
        {error ? (
          <InspectionEmpty>Memory is unavailable. Reopen this panel to retry.</InspectionEmpty>
        ) : (
          <InspectionSkeleton />
        )}
      </div>
    );
  const tags = memoryTagCounts(data.entries);
  const activity = memoryActivityBuckets(data.activity).map((bucket) => ({
    ...bucket,
    name: new Date(bucket.timestamp).toLocaleString([], {
      month: "short",
      day: "numeric",
      hour: "2-digit",
    }),
  }));
  const entries = data.entries.filter(
    (entry) =>
      (!tag || entry.tags.includes(tag)) &&
      (!usedOnly || data.retrieved_keys.includes(entry.key)) &&
      `${entry.key} ${entry.content} ${entry.tags.join(" ")}`
        .toLocaleLowerCase()
        .includes(query.trim().toLocaleLowerCase()),
  );
  const selected = data.entries.find((entry) => entry.key === selectedKey);
  const latestUpdate = Math.max(0, ...data.entries.map((entry) => entry.updated_at));
  const clear = () => {
    setQuery("");
    setTag("");
    setUsedOnly(false);
  };
  return (
    <div className="space-y-5">
      <InspectionError>{error}</InspectionError>
      <InspectionMetrics
        items={[
          { label: "Saved facts", value: data.entries.length },
          { label: "Used in latest request", value: data.retrieved_keys.length },
          { label: "Context compressions", value: data.compression_count },
          {
            label: "Summary",
            value: data.summary ? "Available" : "Not needed",
            hint: data.summary_timestamp_ms
              ? inspectionTime(data.summary_timestamp_ms)
              : "Generated during context compression",
          },
        ]}
      />
      <div className="grid items-start gap-5 xl:grid-cols-2">
        <InspectionCard
          title="Recent memory activity"
          description={`Latest ${data.activity.length} operations, grouped by hour · includes failed attempts`}
        >
          {activity.length ? (
            <>
              {active && (
                <ChartFrame label="Recent memory searches, saves and deletes">
                  <BarChart data={activity}>
                    <CartesianGrid vertical={false} strokeDasharray="3 3" />
                    <XAxis
                      dataKey="name"
                      tick={axisStyle}
                      axisLine={false}
                      tickLine={false}
                      minTickGap={30}
                    />
                    <YAxis
                      tick={axisStyle}
                      axisLine={false}
                      tickLine={false}
                      allowDecimals={false}
                      width={30}
                    />
                    <Tooltip content={<ChartTooltip />} />
                    <Bar
                      dataKey="searches"
                      name="Searches"
                      stackId="memory"
                      fill={chartColors[0]}
                      maxBarSize={32}
                    />
                    <Bar dataKey="writes" name="Saves" stackId="memory" fill={chartColors[1]} />
                    <Bar dataKey="deletes" name="Deletes" stackId="memory" fill={chartColors[2]} />
                    <Bar dataKey="other" name="Other" stackId="memory" fill={chartColors[3]} />
                  </BarChart>
                </ChartFrame>
              )}
              <ChartKey
                items={[
                  { label: "Searches", color: chartColors[0] },
                  { label: "Saves", color: chartColors[1] },
                  { label: "Deletes", color: chartColors[2] },
                  { label: "Other", color: chartColors[3] },
                ]}
              />
            </>
          ) : (
            <InspectionEmpty>No explicit memory operations have been recorded.</InspectionEmpty>
          )}
        </InspectionCard>
        <InspectionCard
          title="Memory by tag"
          description="Top eight tags · a fact can belong to more than one tag"
        >
          {tags.length ? (
            <>
              {active && (
                <ChartFrame label="Saved facts per tag">
                  <BarChart layout="vertical" data={tags} margin={{ right: 20 }}>
                    <CartesianGrid horizontal={false} strokeDasharray="3 3" />
                    <XAxis
                      type="number"
                      tick={axisStyle}
                      tickLine={false}
                      axisLine={false}
                      allowDecimals={false}
                    />
                    <YAxis
                      type="category"
                      dataKey="name"
                      tick={axisStyle}
                      width={100}
                      axisLine={false}
                      tickLine={false}
                      tickFormatter={(value: string) =>
                        value.length > 15 ? `${value.slice(0, 14)}…` : value
                      }
                    />
                    <Tooltip content={<ChartTooltip />} />
                    <Bar
                      dataKey="count"
                      name="Facts"
                      fill={chartColors[1]}
                      radius={[0, 4, 4, 0]}
                      maxBarSize={24}
                      className="cursor-pointer"
                      onClick={(value: unknown) => {
                        const entry = value as { name?: string; payload?: { name?: string } };
                        const name = entry.payload?.name ?? entry.name;
                        if (name) setTag((current) => (current === name ? "" : name));
                      }}
                    />
                  </BarChart>
                </ChartFrame>
              )}
              <p className="mt-3 text-[11px] text-muted-foreground">
                Select a bar to filter saved facts below.
              </p>
            </>
          ) : (
            <InspectionEmpty>Tag distribution appears when saved facts have tags.</InspectionEmpty>
          )}
        </InspectionCard>
      </div>
      <InspectionCard
        title="Session summary"
        description="Compressed conversation history used for continuation"
      >
        {data.summary ? (
          <>
            <details open={data.summary.length < 700}>
              <summary className="cursor-pointer text-xs text-muted-foreground">
                Read summary
              </summary>
              <p className="mt-4 whitespace-pre-wrap break-words text-sm leading-7">
                {data.summary}
              </p>
            </details>
            <p className="mt-4 border-t pt-3 text-[11px] text-muted-foreground">
              Updated {inspectionTime(data.summary_timestamp_ms)} · Window starts at event{" "}
              {data.summarized_events}
            </p>
          </>
        ) : (
          <InspectionEmpty>
            No summary yet. It is created when context compression is needed.
          </InspectionEmpty>
        )}
      </InspectionCard>
      <InspectionCard
        title="Saved memory"
        description={`Explicit facts and preferences · Last updated ${latestUpdate ? inspectionTime(latestUpdate) : "—"}`}
      >
        <div className="mb-5 flex flex-wrap items-center gap-3">
          <div className="relative min-w-0 flex-1 basis-60">
            <SearchIcon
              aria-hidden
              className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              aria-label="Search saved memory"
              placeholder="Search facts, tags, or content…"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="h-9 pl-9"
            />
          </div>
          <select
            aria-label="Filter memory by tag"
            value={tag}
            onChange={(event) => setTag(event.target.value)}
            className="h-9 max-w-48 rounded-lg border bg-background px-3 text-xs"
          >
            <option value="">All tags</option>
            {[...new Set(data.entries.flatMap((entry) => entry.tags))].sort().map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </select>
          <Button
            type="button"
            size="sm"
            variant={usedOnly ? "secondary" : "outline"}
            aria-pressed={usedOnly}
            onClick={() => setUsedOnly(!usedOnly)}
          >
            Recently used
          </Button>
          {(query || tag || usedOnly) && (
            <Button type="button" size="sm" variant="ghost" onClick={clear}>
              <XIcon />
              Clear
            </Button>
          )}
        </div>
        <p className="mb-3 text-xs text-muted-foreground">
          {entries.length} of {data.entries.length} facts
        </p>
        {!entries.length ? (
          <InspectionEmpty>
            {query || tag || usedOnly
              ? "No matching facts. Try clearing the filters."
              : "No long-term facts saved yet. Conversation history is not automatically stored here."}
          </InspectionEmpty>
        ) : (
          <div className="divide-y rounded-xl border">
            {entries.map((entry) => (
              <button
                key={entry.key}
                type="button"
                onClick={() => setSelectedKey(entry.key)}
                className="flex w-full items-start gap-3 px-4 py-4 text-left outline-none transition-colors hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50"
              >
                <BrainIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1">
                  <span className="flex flex-wrap items-center gap-2">
                    <span className="break-words text-sm font-medium">{entry.key}</span>
                    {data.retrieved_keys.includes(entry.key) && (
                      <span className="rounded-md bg-muted px-2 py-0.5 text-[10px] text-muted-foreground">
                        Recently used
                      </span>
                    )}
                  </span>
                  <span className="mt-1.5 line-clamp-2 block text-xs leading-5 text-muted-foreground">
                    {entry.content}
                  </span>
                  <span className="mt-2 flex flex-wrap gap-1.5">
                    {entry.tags.map((value, index) => (
                      <span
                        key={`${value}-${index}`}
                        className="rounded-md border px-1.5 py-0.5 text-[10px] text-muted-foreground"
                      >
                        {value}
                      </span>
                    ))}
                  </span>
                </span>
                <time className="hidden shrink-0 text-[10px] text-muted-foreground sm:block">
                  {inspectionTime(entry.updated_at)}
                </time>
              </button>
            ))}
          </div>
        )}
      </InspectionCard>
      <InspectionCard
        title="Memory operations"
        description="Recent explicit tool calls, including failures and pending calls"
      >
        {!data.activity.length ? (
          <InspectionEmpty>No memory operations recorded.</InspectionEmpty>
        ) : (
          <div className="max-h-80 overflow-y-auto">
            <ol className="divide-y">
              {data.activity.map((event, i) => (
                <li
                  key={`${event.execution_id}-${event.call_id}-${i}`}
                  className="flex items-start justify-between gap-3 py-3"
                >
                  <div className="min-w-0">
                    <p className="break-words text-xs font-medium capitalize">
                      {event.action || "Memory operation"}
                      {event.key ? ` · ${event.key}` : ""}
                    </p>
                    <p className="mt-1 text-[10px] text-muted-foreground">
                      {inspectionTime(event.timestamp * 1000)}
                    </p>
                  </div>
                  <span
                    className={`shrink-0 rounded-md px-2 py-1 text-[10px] ${event.status === "error" ? "bg-destructive/10 text-destructive" : "bg-muted text-muted-foreground"}`}
                  >
                    {event.status}
                  </span>
                </li>
              ))}
            </ol>
          </div>
        )}
      </InspectionCard>
      <Sheet
        open={!!selected}
        onOpenChange={(open) => {
          if (!open) setSelectedKey(undefined);
        }}
      >
        <SheetContent className="gap-0 data-[side=right]:w-full data-[side=right]:sm:max-w-xl">
          <div className="shrink-0 border-b p-6 pr-12">
            <SheetTitle className="break-words text-xl font-semibold">{selected?.key}</SheetTitle>
            <SheetDescription className="mt-2">Saved fact and provenance.</SheetDescription>
          </div>
          <div className="min-h-0 flex-1 space-y-6 overflow-y-auto p-6">
            <p className="whitespace-pre-wrap break-words text-sm leading-7">{selected?.content}</p>
            <dl className="space-y-3 border-t pt-5 text-xs">
              {[
                ["Revision", selected?.revision],
                ["Updated", inspectionTime(selected?.updated_at)],
                ["Source session", selected?.source_session],
                ["Tags", selected?.tags.join(", ") || "None"],
              ].map(([label, value]) => (
                <div key={label} className="grid grid-cols-[100px_1fr] gap-4">
                  <dt className="text-muted-foreground">{label}</dt>
                  <dd className="break-all">{value}</dd>
                </div>
              ))}
            </dl>
          </div>
        </SheetContent>
      </Sheet>
    </div>
  );
}
