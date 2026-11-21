"use client";

import { useId } from "react";
import {
  Area,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  ComposedChart,
  Line,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  contextComponents,
  contextUsage,
  inspectionTime,
  tokenCount,
  type ContextInspection,
  type InspectionAnalytics,
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

export function ContextPanel({
  data,
  loading,
  error,
  analytics,
  active = true,
}: {
  data?: ContextInspection;
  loading: boolean;
  error: string;
  analytics?: InspectionAnalytics;
  active?: boolean;
}) {
  const inputFillId = useId();
  if (!data)
    return (
      <div className="space-y-5">
        <InspectionError>{error}</InspectionError>
        {loading ? (
          <InspectionSkeleton />
        ) : (
          <InspectionEmpty>Context is unavailable.</InspectionEmpty>
        )}
      </div>
    );
  const request = data.latest_request;
  const usage = contextUsage(request);
  const budget = request?.budget ?? data.next_request_budget;
  const components = request
    ? contextComponents(request).map((part) => ({
        name:
          (
            {
              system: "System",
              tools: "Tools",
              memory: "Memory",
              conversation: "Conversation",
            } as Record<string, string>
          )[part.name] ?? part.name,
        tokens: part.estimated_tokens,
      }))
    : [];
  const free =
    budget && usage
      ? Math.max(0, usage.capacity - usage.tokens - budget.output_tokens - budget.safety_tokens)
      : null;
  const allocation =
    budget && usage
      ? [
          {
            name: "Context window",
            input: usage.tokens,
            output: budget.output_tokens,
            safety: budget.safety_tokens,
            free: free!,
          },
        ]
      : [];
  const overflow =
    budget && usage
      ? Math.max(0, usage.tokens + budget.output_tokens + budget.safety_tokens - usage.capacity)
      : 0;
  const history = analytics?.context_requests ?? [];
  const trend: {
    name: string;
    input: number | null;
    capacity: number;
    model: string;
    estimated: boolean;
  }[] = [];
  const modelChanges: string[] = [];
  history.forEach((point, i) => {
    const name = `#${(analytics?.context_request_count ?? history.length) - history.length + i + 1}`;
    if (i && (point.model !== history[i - 1].model || point.capacity !== history[i - 1].capacity)) {
      trend.push({
        name: `gap-${i}`,
        input: null,
        capacity: point.capacity,
        model: point.model,
        estimated: point.estimated,
      });
      modelChanges.push(name);
    }
    trend.push({
      name,
      input: point.input_tokens,
      capacity: point.capacity,
      model: point.model,
      estimated: point.estimated,
    });
  });
  const compressionMarkers = (analytics?.compactions ?? [])
    .filter((point) => history.length && point.timestamp_ms >= history[0].timestamp_ms)
    .slice(-6)
    .flatMap((point) => {
      const i = history.findIndex((request) => request.timestamp_ms >= point.timestamp_ms);
      return i < 0
        ? []
        : [`#${(analytics?.context_request_count ?? history.length) - history.length + i + 1}`];
    });
  const compact = data.last_compaction;
  const saved = compact ? compact.before_tokens - compact.after_tokens : 0;
  return (
    <div className="space-y-5">
      <InspectionError>{error || data.budget_error}</InspectionError>
      <InspectionMetrics
        items={[
          {
            label: "Context utilization",
            value: usage ? `${usage.percent}%` : "—",
            hint: usage
              ? usage.estimated
                ? "Estimated input"
                : "Provider-reported input"
              : "No captured request",
          },
          {
            label: "Input tokens",
            value: usage ? `${usage.estimated ? "~" : ""}${tokenCount(usage.tokens)}` : "—",
          },
          { label: "Available after reservations", value: free !== null ? tokenCount(free) : "—" },
          { label: "Context compressions", value: data.compression_count.toLocaleString() },
        ]}
      />
      <div className="grid items-start gap-5 xl:grid-cols-2">
        <InspectionCard
          title="Window allocation"
          description={
            request
              ? `${request.budget.model} · Step ${request.step} · ${inspectionTime(request.timestamp_ms)}`
              : "Latest prepared model request"
          }
        >
          {usage && budget ? (
            <>
              <div className="mb-5 flex items-baseline gap-2">
                <strong className="text-3xl font-semibold tracking-tight tabular-nums">
                  {tokenCount(usage.tokens)}
                </strong>
                <span className="text-sm text-muted-foreground">
                  / {tokenCount(usage.capacity)} tokens
                </span>
              </div>
              {active && (
                <ChartFrame
                  label="Context window: input, reserved output, safety and remaining capacity"
                  height={115}
                >
                  <BarChart
                    layout="vertical"
                    data={allocation}
                    margin={{ left: 0, right: 0, top: 4, bottom: 0 }}
                  >
                    <XAxis
                      type="number"
                      tickFormatter={tokenCount}
                      tick={axisStyle}
                      axisLine={false}
                      tickLine={false}
                    />
                    <YAxis type="category" dataKey="name" hide />
                    <Tooltip content={<ChartTooltip tokens />} cursor={false} />
                    <Bar
                      dataKey="input"
                      name="Input"
                      stackId="window"
                      fill={chartColors[0]}
                      barSize={32}
                    />
                    <Bar
                      dataKey="output"
                      name="Reserved output"
                      stackId="window"
                      fill={chartColors[1]}
                    />
                    <Bar
                      dataKey="safety"
                      name="Safety margin"
                      stackId="window"
                      fill={chartColors[3]}
                    />
                    <Bar
                      dataKey="free"
                      name="Available"
                      stackId="window"
                      fill="var(--muted)"
                      radius={[0, 4, 4, 0]}
                    />
                    {overflow > 0 && (
                      <ReferenceLine x={usage.capacity} stroke="var(--destructive)" />
                    )}
                  </BarChart>
                </ChartFrame>
              )}
              <ChartKey
                items={[
                  { label: "Input", color: chartColors[0], value: tokenCount(usage.tokens) },
                  {
                    label: "Output reserve",
                    color: chartColors[1],
                    value: tokenCount(budget.output_tokens),
                  },
                  {
                    label: "Safety",
                    color: chartColors[3],
                    value: tokenCount(budget.safety_tokens),
                  },
                  { label: "Available", color: "var(--muted)", value: tokenCount(free!) },
                ]}
              />
              {overflow > 0 && (
                <p className="mt-3 text-xs text-destructive">
                  Input and reservations exceed capacity by {tokenCount(overflow)} tokens.
                </p>
              )}
              <p className="mt-4 text-[11px] leading-5 text-muted-foreground">
                Captured request budget. Reserved output is capacity reserved for a reply, not
                tokens already generated.
              </p>
            </>
          ) : (
            <InspectionEmpty>Send a message to capture the first model request.</InspectionEmpty>
          )}
        </InspectionCard>
        <InspectionCard
          title="Input composition"
          description="Estimated categories allocated to the input total"
        >
          {components.length ? (
            <>
              {active && (
                <ChartFrame label="Input token estimates by category">
                  <BarChart layout="vertical" data={components} margin={{ left: 0, right: 20 }}>
                    <CartesianGrid horizontal={false} strokeDasharray="3 3" />
                    <XAxis
                      type="number"
                      tickFormatter={tokenCount}
                      tick={axisStyle}
                      axisLine={false}
                      tickLine={false}
                    />
                    <YAxis
                      type="category"
                      dataKey="name"
                      tick={axisStyle}
                      width={95}
                      axisLine={false}
                      tickLine={false}
                    />
                    <Tooltip
                      content={<ChartTooltip tokens />}
                      cursor={{ fill: "var(--muted)", opacity: 0.5 }}
                    />
                    <Bar
                      dataKey="tokens"
                      name="Estimated input"
                      fill={chartColors[1]}
                      radius={[0, 4, 4, 0]}
                      maxBarSize={24}
                    />
                  </BarChart>
                </ChartFrame>
              )}
              <p className="mt-4 text-[11px] leading-5 text-muted-foreground">
                Providers report the total. Category weights are local estimates, not separate
                provider measurements.
              </p>
            </>
          ) : (
            <InspectionEmpty>No input breakdown is available yet.</InspectionEmpty>
          )}
        </InspectionCard>
      </div>
      <InspectionCard
        title="Context over time"
        description={`Latest ${history.length} of ${analytics?.context_request_count ?? 0} captured requests · gaps mark model or capacity changes`}
      >
        {history.length ? (
          <>
            {active && (
              <ChartFrame
                label="Input tokens by model request, with compression and model changes"
                height={260}
              >
                <ComposedChart data={trend} margin={{ left: 0, right: 15, top: 15 }}>
                  <defs>
                    <linearGradient id={inputFillId} x1="0%" y1="0%" x2="0%" y2="100%">
                      <stop offset="0%" stopColor={chartColors[0]} stopOpacity={0.2} />
                      <stop offset="100%" stopColor={chartColors[0]} stopOpacity={0.02} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid vertical={false} strokeDasharray="3 3" />
                  <XAxis
                    dataKey="name"
                    tick={axisStyle}
                    tickLine={false}
                    axisLine={false}
                    minTickGap={26}
                    tickFormatter={(name: string) => (name.startsWith("gap-") ? "" : name)}
                  />
                  <YAxis
                    tickFormatter={tokenCount}
                    tick={axisStyle}
                    axisLine={false}
                    tickLine={false}
                    width={52}
                  />
                  <Tooltip content={<ChartTooltip tokens />} />
                  <Area
                    dataKey="input"
                    name="Input"
                    type="linear"
                    stroke={chartColors[0]}
                    strokeWidth={2}
                    fill={`url(#${inputFillId})`}
                    dot={history.length < 20 ? { r: 3 } : false}
                    connectNulls={false}
                    isAnimationActive={false}
                  />
                  <Line
                    dataKey="capacity"
                    name="Capacity"
                    stroke={chartColors[3]}
                    strokeDasharray="4 4"
                    dot={false}
                    isAnimationActive={false}
                  />
                  {compressionMarkers.map((name, i) => (
                    <ReferenceLine
                      key={`compression-${name}-${i}`}
                      x={name}
                      stroke={chartColors[2]}
                      strokeDasharray="3 3"
                      label={{
                        value: "Compressed",
                        position: "insideTopLeft",
                        fill: "var(--muted-foreground)",
                        fontSize: 10,
                      }}
                    />
                  ))}
                  {modelChanges.slice(-4).map((name) => (
                    <ReferenceLine
                      key={`model-${name}`}
                      x={name}
                      stroke={chartColors[4]}
                      label={{
                        value: "Model / budget",
                        position: "insideBottomLeft",
                        fill: "var(--muted-foreground)",
                        fontSize: 10,
                      }}
                    />
                  ))}
                </ComposedChart>
              </ChartFrame>
            )}
            <ChartKey
              items={[
                { label: "Input tokens", color: chartColors[0] },
                { label: "Window capacity", color: chartColors[3] },
              ]}
            />
          </>
        ) : (
          <InspectionEmpty>
            {analytics
              ? "Request history will appear after committed model requests. Refresh charts to load new checkpoints."
              : "Chart history has not loaded. Use Refresh charts to retry."}
          </InspectionEmpty>
        )}
      </InspectionCard>
      <div className="grid items-start gap-5 xl:grid-cols-2">
        <InspectionCard
          title="Latest compression"
          description={
            compact
              ? inspectionTime(compact.timestamp_ms)
              : "History is compressed when the context window needs space"
          }
        >
          {compact ? (
            <>
              <p className="mb-4 text-sm font-medium">
                {saved >= 0
                  ? `~${tokenCount(saved)} tokens saved`
                  : `~${tokenCount(-saved)} tokens added`}
                {compact.before_tokens > 0 && (
                  <span className="ml-2 text-xs font-normal text-muted-foreground">
                    ({((saved / compact.before_tokens) * 100).toFixed(1)}%)
                  </span>
                )}
              </p>
              {active && (
                <ChartFrame
                  label="Estimated token counts before and after compression"
                  height={155}
                >
                  <BarChart
                    layout="vertical"
                    data={[
                      { name: "Before", tokens: compact.before_tokens },
                      { name: "After", tokens: compact.after_tokens },
                    ]}
                  >
                    <XAxis
                      type="number"
                      tickFormatter={tokenCount}
                      tick={axisStyle}
                      axisLine={false}
                      tickLine={false}
                    />
                    <YAxis
                      type="category"
                      dataKey="name"
                      tick={axisStyle}
                      axisLine={false}
                      tickLine={false}
                      width={50}
                    />
                    <Tooltip content={<ChartTooltip tokens />} />
                    <Bar
                      dataKey="tokens"
                      name="Estimated tokens"
                      fill={chartColors[1]}
                      radius={[0, 4, 4, 0]}
                      maxBarSize={24}
                    />
                  </BarChart>
                </ChartFrame>
              )}
              <p className="mt-3 text-[11px] text-muted-foreground">
                Local estimates before and after the latest compression.
              </p>
            </>
          ) : (
            <InspectionEmpty>No context compression has been needed yet.</InspectionEmpty>
          )}
        </InspectionCard>
        <InspectionCard
          title="Session token usage"
          description="Provider-reported totals, including delegated work"
        >
          {active && (
            <ChartFrame
              label="Cumulative session input and output tokens"
              height={compact ? 190 : 155}
            >
              <BarChart
                layout="vertical"
                data={[
                  { name: "Input", tokens: data.cumulative_input_tokens },
                  { name: "Output", tokens: data.cumulative_output_tokens },
                ]}
              >
                <XAxis
                  type="number"
                  tickFormatter={tokenCount}
                  tick={axisStyle}
                  axisLine={false}
                  tickLine={false}
                />
                <YAxis
                  type="category"
                  dataKey="name"
                  tick={axisStyle}
                  axisLine={false}
                  tickLine={false}
                  width={50}
                />
                <Tooltip content={<ChartTooltip tokens />} />
                <Bar
                  dataKey="tokens"
                  name="Reported tokens"
                  fill={chartColors[0]}
                  radius={[0, 4, 4, 0]}
                  maxBarSize={24}
                >
                  <Cell fill={chartColors[0]} />
                  <Cell fill={chartColors[1]} />
                </Bar>
              </BarChart>
            </ChartFrame>
          )}
          <ChartKey
            items={[
              {
                label: "Input",
                color: chartColors[0],
                value: tokenCount(data.cumulative_input_tokens),
              },
              {
                label: "Output",
                color: chartColors[1],
                value: tokenCount(data.cumulative_output_tokens),
              },
            ]}
          />
          <p className="mt-4 text-[11px] leading-5 text-muted-foreground">
            Totals include repeated requests and are separate from current window occupancy.
            Requests without usage reports are excluded.
          </p>
        </InspectionCard>
      </div>
      {budget && (
        <details className="rounded-2xl border bg-card px-5 py-4">
          <summary className="cursor-pointer text-sm font-medium">
            Context policy and model details
          </summary>
          <dl className="mt-4 grid gap-3 text-xs sm:grid-cols-2">
            {[
              ["Captured model", budget.model],
              ["Input / compaction limit", tokenCount(budget.input_limit)],
              ["Sliding window target", tokenCount(budget.sliding_window_tokens)],
              ["Window starts at event", data.window_start],
              ["Recorded events", data.total_events],
              ["Next selected model", data.next_request_budget?.model ?? "Unavailable"],
            ].map(([name, value]) => (
              <div key={name} className="flex justify-between gap-4 border-b pb-2">
                <dt className="text-muted-foreground">{name}</dt>
                <dd className="break-all text-right font-medium">{value}</dd>
              </div>
            ))}
          </dl>
        </details>
      )}
    </div>
  );
}
