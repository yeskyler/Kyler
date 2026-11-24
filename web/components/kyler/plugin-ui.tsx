"use client";

import type { ReactNode } from "react";
import { Menu } from "@base-ui/react/menu";
import { ChevronDownIcon, EllipsisIcon, SearchIcon, XIcon, PackageIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { pluginStatuses } from "@/lib/kyler/plugins";

export const pluginGrid = "grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3";
export const pluginCard =
  "group flex min-w-0 flex-col rounded-2xl border border-border/80 bg-card p-5 transition-colors hover:border-foreground/20";
export const pluginIcon =
  "flex size-11 shrink-0 items-center justify-center rounded-xl border bg-muted/50 text-muted-foreground";
export const pluginMenuItem =
  "flex w-full cursor-default items-center gap-2 rounded-md px-2.5 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[disabled]:opacity-40";

export type PluginPanelProps = {
  active: boolean;
  addSignal: number;
  onCount: (count: number) => void;
};
export function StatusPill({ status }: { status: string }) {
  const tone = ["enabled", "connected"].includes(status)
    ? "bg-emerald-500"
    : status === "needs_approval" || status === "connecting"
      ? "bg-amber-500"
      : ["failed", "invalid"].includes(status)
        ? "bg-destructive"
        : "bg-muted-foreground/45";
  return (
    <span className="inline-flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
      <span
        aria-hidden
        className={`size-1.5 rounded-full ${tone} ${status === "connecting" ? "animate-pulse" : ""}`}
      />
      {pluginStatuses[status] ?? status.replaceAll("_", " ")}
    </span>
  );
}
export function SourceBadge({ children }: { children: ReactNode }) {
  return (
    <span className="rounded-md border bg-muted/30 px-2 py-0.5 text-[11px] text-muted-foreground">
      {children}
    </span>
  );
}
export function PluginActions({
  label,
  children,
  disabled,
}: {
  label: string;
  children: ReactNode;
  disabled?: boolean;
}) {
  return (
    <Menu.Root>
      <Menu.Trigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`Actions for ${label}`}
            disabled={disabled}
          />
        }
      >
        <EllipsisIcon className="size-4" />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={5} className="z-50">
          <Menu.Popup
            aria-label={`Actions for ${label}`}
            className="min-w-40 rounded-xl border bg-popover p-1 text-popover-foreground shadow-lg outline-none"
          >
            {children}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
export function PluginToolbar({
  query,
  onQuery,
  source,
  onSource,
  sources,
  status,
  onStatus,
  statuses,
  kind,
  showSource = true,
}: {
  showSource?: boolean;
  query: string;
  onQuery: (query: string) => void;
  source: string;
  onSource: (source: string) => void;
  sources: Record<string, string>;
  status: string;
  onStatus: (status: string) => void;
  statuses: string[];
  kind: string;
}) {
  return (
    <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
      <div className="relative min-w-0 flex-1">
        <SearchIcon
          aria-hidden
          className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          value={query}
          onChange={(event) => onQuery(event.target.value)}
          placeholder={`Search ${kind.toLowerCase()}…`}
          aria-label={`Search ${kind.toLowerCase()}`}
          className="h-9 pl-9 pr-9 shadow-none"
        />
        {query && (
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label="Clear search"
            className="absolute right-1.5 top-1/2 -translate-y-1/2"
            onClick={() => onQuery("")}
          >
            <XIcon />
          </Button>
        )}
      </div>
      <div className="flex shrink-0 gap-2">
        {[
          { label: "Source", value: source, change: onSource, options: sources },
          {
            label: "Status",
            value: status,
            change: onStatus,
            options: Object.fromEntries(
              statuses.map((value) => [value, pluginStatuses[value] ?? value]),
            ),
          },
        ]
          .filter(({ label }) => showSource || label !== "Source")
          .map(({ label, value, change, options }) => (
            <div key={label} className="relative min-w-0 flex-1 sm:flex-none">
              <select
                aria-label={`Filter ${kind.toLowerCase()} by ${label.toLowerCase()}`}
                value={value}
                onChange={(event) => change(event.target.value)}
                className="h-9 w-full appearance-none rounded-lg border border-input bg-background pl-3 pr-8 text-xs text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring/50 sm:w-auto"
              >
                <option value="all">All {label === "Status" ? "statuses" : "sources"}</option>
                {Object.entries(options).map(([id, name]) => (
                  <option key={id} value={id}>
                    {name}
                  </option>
                ))}
              </select>
              <ChevronDownIcon
                aria-hidden
                className="pointer-events-none absolute right-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
              />
            </div>
          ))}
      </div>
    </div>
  );
}
export function PluginEmpty({
  filtered,
  kind,
  onAdd,
  onClear,
}: {
  filtered: boolean;
  kind: "skills" | "connectors";
  onAdd: () => void;
  onClear: () => void;
}) {
  return (
    <div className="flex min-h-72 flex-col items-center justify-center rounded-2xl border border-dashed px-6 py-12 text-center">
      <div className="mb-4 rounded-2xl bg-muted/70 p-4">
        <PackageIcon className="size-6 text-muted-foreground" />
      </div>
      <h3 className="font-medium">
        {filtered
          ? "No matching plugins"
          : `Add your first ${kind === "skills" ? "skill" : "connector"}`}
      </h3>
      <p className="mt-2 max-w-sm text-sm leading-relaxed text-muted-foreground">
        {filtered
          ? "Try another search or clear your filters."
          : kind === "skills"
            ? "Install a skill to give your agent a reusable workflow."
            : "Connect external tools and services to your agent."}
      </p>
      <Button type="button" variant="outline" className="mt-5" onClick={filtered ? onClear : onAdd}>
        {filtered ? "Clear filters" : kind === "skills" ? "Add skill" : "Add connector"}
      </Button>
    </div>
  );
}
export function PluginSkeletons() {
  return (
    <div className={pluginGrid} role="status" aria-label="Loading plugins">
      <span className="sr-only">Loading plugins…</span>
      {Array.from({ length: 6 }, (_, i) => (
        <div key={i} className="space-y-5 rounded-2xl border p-5">
          <div className="flex items-center gap-3">
            <Skeleton className="size-11 rounded-xl" />
            <Skeleton className="h-4 w-28" />
          </div>
          <div className="space-y-2">
            <Skeleton className="h-3 w-full" />
            <Skeleton className="h-3 w-3/4" />
          </div>
          <Skeleton className="h-7 w-24" />
        </div>
      ))}
    </div>
  );
}
export function PluginNotice({ error, notice }: { error: string; notice: string }) {
  return error ? (
    <p
      role="alert"
      className="rounded-xl border border-destructive/20 bg-destructive/5 px-4 py-3 text-sm text-destructive"
    >
      {error}
    </p>
  ) : notice ? (
    <p
      role="status"
      className="rounded-xl border bg-muted/30 px-4 py-3 text-sm text-muted-foreground"
    >
      {notice}
    </p>
  ) : null;
}
