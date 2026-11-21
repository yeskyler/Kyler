"use client";

import { useState } from "react";
import { Bar, BarChart, CartesianGrid, Tooltip, XAxis, YAxis } from "recharts";
import type { InspectionAnalytics } from "@/lib/kyler/inspection";
import {
  ChartFrame,
  ChartKey,
  ChartTooltip,
  InspectionCard,
  InspectionEmpty,
  axisStyle,
  chartColors,
} from "./inspection-ui";

const series = [
  { key: "messages", name: "Messages", color: chartColors[1] },
  { key: "tools", name: "Tool starts", color: chartColors[0] },
  { key: "compactions", name: "Compressions", color: chartColors[2] },
  { key: "subagents", name: "Subagent updates", color: chartColors[3] },
] as const;
const toolResults = [
  { key: "succeeded", name: "Succeeded", color: chartColors[1] },
  { key: "failed", name: "Failed", color: "var(--destructive)" },
  { key: "pending", name: "No recorded result", color: chartColors[2] },
];

export function RolloutCharts({
  analytics,
  active,
  onTool,
}: {
  analytics?: InspectionAnalytics;
  active: boolean;
  onTool: (name: string) => void;
}) {
  const [hidden, setHidden] = useState<string[]>([]);
  const activity =
    analytics?.activity.map((bucket) => ({
      ...bucket,
      name: new Date(bucket.timestamp_ms).toLocaleString([], {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      }),
    })) ?? [];
  return (
    <div className="grid items-start gap-5 xl:grid-cols-2">
      <InspectionCard
        title="Committed activity"
        description="Complete session · messages, tool starts, compressions and subagent updates"
      >
        {activity.length ? (
          <>
            {active && hidden.length < series.length ? (
              <ChartFrame label="Session activity grouped by time">
                <BarChart data={activity}>
                  <CartesianGrid vertical={false} strokeDasharray="3 3" />
                  <XAxis
                    dataKey="name"
                    tick={axisStyle}
                    axisLine={false}
                    tickLine={false}
                    minTickGap={28}
                  />
                  <YAxis
                    tick={axisStyle}
                    axisLine={false}
                    tickLine={false}
                    allowDecimals={false}
                    width={30}
                  />
                  <Tooltip content={<ChartTooltip />} />
                  {series
                    .filter((item) => !hidden.includes(item.key))
                    .map((item) => (
                      <Bar
                        key={item.key}
                        dataKey={item.key}
                        name={item.name}
                        stackId="activity"
                        fill={item.color}
                        maxBarSize={30}
                      />
                    ))}
                </BarChart>
              </ChartFrame>
            ) : (
              active && <InspectionEmpty>Select an activity category below.</InspectionEmpty>
            )}
            <div className="mt-4 flex flex-wrap gap-x-4 gap-y-2">
              {series.map((item) => (
                <button
                  type="button"
                  key={item.key}
                  aria-pressed={!hidden.includes(item.key)}
                  onClick={() =>
                    setHidden((values) =>
                      values.includes(item.key)
                        ? values.filter((value) => value !== item.key)
                        : [...values, item.key],
                    )
                  }
                  className={`inline-flex items-center gap-1.5 rounded text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50 ${hidden.includes(item.key) ? "text-muted-foreground/40 line-through" : "text-muted-foreground"}`}
                >
                  <span className="size-2 rounded-sm" style={{ background: item.color }} />
                  {item.name}
                </button>
              ))}
            </div>
          </>
        ) : (
          <InspectionEmpty>
            {analytics
              ? "Committed activity appears after a task starts."
              : "Chart history has not loaded. Use Refresh charts to retry."}
          </InspectionEmpty>
        )}
      </InspectionCard>
      <InspectionCard
        title="Tool calls"
        description="Complete session · top 20 tools, grouped by recorded result"
      >
        {analytics?.tools.length ? (
          <>
            {active && (
              <ChartFrame
                label="Tool call counts by success, failure and unresolved result"
                height={Math.max(230, Math.min(600, analytics.tools.length * 34 + 35))}
              >
                <BarChart layout="vertical" data={analytics.tools} margin={{ right: 15 }}>
                  <CartesianGrid horizontal={false} strokeDasharray="3 3" />
                  <XAxis
                    type="number"
                    tick={axisStyle}
                    axisLine={false}
                    tickLine={false}
                    allowDecimals={false}
                  />
                  <YAxis
                    type="category"
                    dataKey="name"
                    tick={axisStyle}
                    axisLine={false}
                    tickLine={false}
                    width={112}
                    tickFormatter={(value: string) =>
                      value.length > 18 ? `${value.slice(0, 17)}…` : value
                    }
                  />
                  <Tooltip content={<ChartTooltip />} />
                  {toolResults.map((item) => (
                    <Bar
                      key={item.key}
                      dataKey={item.key}
                      name={item.name}
                      fill={item.color}
                      stackId="calls"
                      maxBarSize={22}
                      className="cursor-pointer"
                      onClick={(value: unknown) => {
                        const entry = value as { name?: string; payload?: { name?: string } };
                        const name = entry.payload?.name ?? entry.name;
                        if (name && name !== "Other tools") onTool(name);
                      }}
                    />
                  ))}
                </BarChart>
              </ChartFrame>
            )}
            <ChartKey items={toolResults.map(({ name, color }) => ({ label: name, color }))} />
            <div className="mt-4 flex flex-wrap gap-2">
              {analytics.tools
                .slice(0, 5)
                .filter((tool) => tool.name !== "Other tools")
                .map((tool) => (
                  <button
                    type="button"
                    key={tool.name}
                    className="max-w-full truncate rounded-md border px-2 py-1 text-[10px] text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/50"
                    onClick={() => onTool(tool.name)}
                  >
                    {tool.name}
                  </button>
                ))}
            </div>
            <p className="mt-3 text-[11px] text-muted-foreground">
              Select a tool to filter loaded timeline records. Missing results are not assumed to be
              running.
            </p>
          </>
        ) : (
          <InspectionEmpty>
            {analytics
              ? "No committed tool calls yet."
              : "Tool statistics have not loaded. Use Refresh charts to retry."}
          </InspectionEmpty>
        )}
      </InspectionCard>
    </div>
  );
}
