"use client";

import type { ReactElement, ReactNode } from "react";
import { ResponsiveContainer } from "recharts";
import { Skeleton } from "@/components/ui/skeleton";
import { tokenCount } from "@/lib/kyler/inspection";

export const chartColors = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
];
export const axisStyle = { fontSize: 11, fill: "var(--muted-foreground)" };
export function InspectionCard({
  title,
  description,
  children,
  className = "",
  action,
}: {
  title: string;
  description?: string;
  children: ReactNode;
  className?: string;
  action?: ReactNode;
}) {
  return (
    <section
      className={`min-w-0 rounded-2xl border border-border/70 bg-card p-5 sm:p-6 ${className}`}
    >
      <div className="mb-5 flex items-start justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold tracking-tight">{title}</h2>
          {description && (
            <p className="mt-1 text-xs leading-5 text-muted-foreground">{description}</p>
          )}
        </div>
        {action}
      </div>
      {children}
    </section>
  );
}
export function InspectionEmpty({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-32 items-center justify-center rounded-xl border border-dashed px-5 py-8 text-center text-sm leading-relaxed text-muted-foreground">
      {children}
    </div>
  );
}
export function InspectionError({ children }: { children: ReactNode }) {
  return children ? (
    <p
      role="alert"
      className="rounded-xl border border-destructive/20 bg-destructive/5 px-4 py-3 text-sm text-destructive"
    >
      {children}
    </p>
  ) : null;
}
export function InspectionMetrics({
  items,
}: {
  items: { label: string; value: ReactNode; hint?: string }[];
}) {
  return (
    <dl className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      {items.map((item) => (
        <div
          key={item.label}
          className="min-w-0 rounded-xl border border-border/70 bg-card px-4 py-4"
        >
          <dt className="text-xs text-muted-foreground">{item.label}</dt>
          <dd className="mt-2 break-words text-2xl font-semibold tracking-tight tabular-nums">
            {item.value}
          </dd>
          {item.hint && (
            <p className="mt-1.5 text-[11px] leading-5 text-muted-foreground">{item.hint}</p>
          )}
        </div>
      ))}
    </dl>
  );
}
export function InspectionSkeleton() {
  return (
    <div role="status" aria-label="Loading inspection" className="space-y-5">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {[0, 1, 2, 3].map((i) => (
          <Skeleton key={i} className="h-24 rounded-xl" />
        ))}
      </div>
      <Skeleton className="h-72 rounded-2xl" />
      <Skeleton className="h-56 rounded-2xl" />
    </div>
  );
}
export function ChartFrame({
  label,
  children,
  height = 230,
}: {
  label: string;
  children: ReactElement;
  height?: number;
}) {
  return (
    <div
      role="img"
      aria-label={label}
      className="min-w-0 [&_.recharts-cartesian-grid_line]:stroke-border/50 [&_.recharts-tooltip-cursor]:fill-muted/40 [&_.recharts-cartesian-axis-tick_text]:fill-muted-foreground"
      style={{ height }}
    >
      <ResponsiveContainer
        width="100%"
        height="100%"
        minWidth={1}
        initialDimension={{ width: 400, height }}
      >
        {children}
      </ResponsiveContainer>
    </div>
  );
}
export function ChartTooltip({
  active,
  payload,
  label,
  tokens = false,
}: {
  active?: boolean;
  payload?: ReadonlyArray<{
    name?: string | number;
    value?: unknown;
    color?: string;
    payload?: Record<string, unknown>;
  }>;
  label?: unknown;
  tokens?: boolean;
}) {
  if (!active || !payload?.length) return null;
  const model = payload[0]?.payload?.model;
  return (
    <div className="max-w-72 rounded-xl border bg-popover px-3 py-2.5 text-xs text-popover-foreground shadow-lg">
      <p className="mb-2 break-words font-medium">
        {typeof label === "string" || typeof label === "number" ? label : "Details"}
      </p>
      {typeof model === "string" && (
        <p className="mb-2 break-words text-muted-foreground">
          {model}
          {payload[0]?.payload?.estimated ? " · estimated" : ""}
        </p>
      )}
      <div className="space-y-1.5">
        {payload
          .filter((entry) => typeof entry.value === "number")
          .map((entry, i) => (
            <div key={`${entry.name}-${i}`} className="flex items-center justify-between gap-6">
              <span className="inline-flex items-center gap-2 text-muted-foreground">
                <span className="size-2 rounded-sm" style={{ backgroundColor: entry.color }} />
                {entry.name}
              </span>
              <span className="font-medium tabular-nums">
                {tokens ? `${tokenCount(entry.value as number)} tokens` : String(entry.value)}
              </span>
            </div>
          ))}
      </div>
    </div>
  );
}
export function ChartKey({ items }: { items: { label: string; color: string; value?: string }[] }) {
  return (
    <div className="mt-4 flex flex-wrap gap-x-5 gap-y-2 text-xs text-muted-foreground">
      {items.map((item) => (
        <span key={item.label} className="inline-flex items-center gap-1.5">
          <span className="size-2 rounded-sm" style={{ backgroundColor: item.color }} />
          {item.label}
          {item.value && <span className="ml-1 text-foreground tabular-nums">{item.value}</span>}
        </span>
      ))}
    </div>
  );
}
