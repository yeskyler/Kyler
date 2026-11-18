"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import {
  ArrowUpRightIcon,
  CalendarClockIcon,
  ClockIcon,
  EllipsisIcon,
  FolderIcon,
  LoaderCircleIcon,
  PauseIcon,
  PlayIcon,
  PlusIcon,
  RefreshCwIcon,
  SearchIcon,
  Settings2Icon,
  SquareIcon,
  Trash2Icon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { Sheet, SheetContent, SheetTitle, SheetDescription } from "@/components/ui/sheet";
import { api } from "@/lib/kyler/client";
import { randomUuid } from "@/lib/kyler/random-uuid";
import type { ModelCatalog, Workspace } from "@/lib/kyler/types";
import {
  mergeRuns,
  runLabels,
  scheduleLabel,
  taskTime,
  unfinished,
  workspaceLabel,
  type Automation,
  type AutomationOptions,
  type AutomationRun,
  type RunPage,
  type RunStatus,
} from "@/lib/kyler/automations";
import { AutomationForm } from "./automation-form";
const menuItem =
  "flex w-full cursor-default items-center gap-2 rounded-md px-2.5 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[disabled]:opacity-40";
function RunBadge({ status, stopping = false }: { status: RunStatus; stopping?: boolean }) {
  return (
    <span
      className={`inline-flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-[10px] font-medium ${["failed", "interrupted"].includes(status) ? "bg-destructive/10 text-destructive" : status === "completed" ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400" : "bg-muted text-muted-foreground"}`}
    >
      {unfinished(status) && <span className="size-1.5 animate-pulse rounded-full bg-current" />}
      {stopping ? "Stopping…" : runLabels[status]}
    </span>
  );
}
export function AutomationsPanel({
  active = true,
  defaultWorkspace,
  onOpenSession,
}: {
  active?: boolean;
  defaultWorkspace?: string;
  onOpenSession: (id: string) => void;
}) {
  const [tasks, setTasks] = useState<Automation[]>([]);
  const [options, setOptions] = useState<AutomationOptions>();
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [online, setOnline] = useState(false);
  const [error, setError] = useState("");
  const [optionsError, setOptionsError] = useState("");
  const [notice, setNotice] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [editing, setEditing] = useState<Automation | null | undefined>();
  const [selectedId, setSelectedId] = useState<string>();
  const selectedRef = useRef(selectedId);
  selectedRef.current = selectedId;
  const [runs, setRuns] = useState<AutomationRun[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [historyError, setHistoryError] = useState("");
  const [historyBusy, setHistoryBusy] = useState(false);
  const [busy, setBusy] = useState("");
  const [removing, setRemoving] = useState<Automation>();
  const runKeys = useRef(new Map<string, string>());
  const selected = tasks.find((task) => task.id === selectedId);
  const operation = useRef<AbortController | null>(null);
  const taskSnapshot = useRef(tasks);
  taskSnapshot.current = tasks;
  const refresh = useCallback(async () => {
    operation.current?.abort();
    const controller = new AbortController();
    operation.current = controller;
    setRefreshing(true);
    try {
      const [items, status] = await Promise.all([
        api<Automation[]>("automations", { signal: controller.signal }),
        api<{ running: boolean }>("automations/status", { signal: controller.signal }),
      ]);
      if (!controller.signal.aborted) {
        setTasks(items);
        setOnline(status.running);
        setError("");
      }
    } catch (failure) {
      if (!controller.signal.aborted) setError((failure as Error).message);
    } finally {
      if (!controller.signal.aborted) {
        setLoading(false);
        setRefreshing(false);
      }
    }
  }, []);
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    void refresh();
    setOptionsError("");
    Promise.all([
      api<ModelCatalog>("models", { signal: controller.signal }),
      api<Workspace[]>("workspaces", { signal: controller.signal }),
      api<{ default_workspace?: string; cwd?: string }>("info", {
        signal: controller.signal,
      }),
    ])
      .then(([models, projects, info]) => {
        if (controller.signal.aborted) return;
        const root = defaultWorkspace ?? info.default_workspace ?? info.cwd ?? "";
        const all = [
          ...(root ? [{ path: root, title: "Default workspace" }] : []),
          ...projects.map((workspace) => ({
            path: workspace.path,
            title: workspace.title,
          })),
        ];
        setOptions({
          models,
          workspaces: [...new Map(all.map((entry) => [entry.path, entry])).values()],
          defaultWorkspace: root || all[0]?.path || "",
        });
      })
      .catch((failure: Error) => {
        if (!controller.signal.aborted) setOptionsError(failure.message);
      });
    let last = Date.now();
    const poll = () => {
      if (document.visibilityState === "hidden") return;
      const needs = taskSnapshot.current.some(
        (task) =>
          task.has_unfinished_run ||
          (task.enabled && task.next_run_at != null && task.next_run_at < Date.now() + 10000),
      );
      if (needs || Date.now() - last >= 60000) {
        last = Date.now();
        void refresh();
      }
    };
    const focus = () => void refresh();
    const timer = setInterval(poll, 5000);
    window.addEventListener("focus", focus);
    return () => {
      controller.abort();
      operation.current?.abort();
      clearInterval(timer);
      window.removeEventListener("focus", focus);
    };
  }, [active, defaultWorkspace, refresh]);
  useEffect(() => {
    if (!selectedId || !active) return;
    const controller = new AbortController();
    let loading = false;
    let initialized = false;
    setRuns([]);
    setCursor(null);
    setHistoryError("");
    setHistoryBusy(true);
    async function refreshHistory() {
      if (loading || document.visibilityState === "hidden") return;
      loading = true;
      try {
        const page = await api<RunPage>(`automations/${selectedId}/runs`, {
          signal: controller.signal,
        });
        if (!controller.signal.aborted) {
          setRuns((previous) => mergeRuns(previous, page.runs));
          if (!initialized) {
            setCursor(page.next_cursor);
            initialized = true;
          }
          setHistoryError("");
        }
      } catch (failure) {
        if (!controller.signal.aborted) setHistoryError((failure as Error).message);
      } finally {
        loading = false;
        if (!controller.signal.aborted) setHistoryBusy(false);
      }
    }
    void refreshHistory();
    const timer = setInterval(refreshHistory, 5000);
    return () => {
      controller.abort();
      clearInterval(timer);
    };
  }, [selectedId, active]);
  async function runNow(task: Automation) {
    if (busy || !online || task.has_unfinished_run) return;
    setBusy(task.id);
    setError("");
    setNotice("");
    const key = runKeys.current.get(task.id) ?? randomUuid();
    runKeys.current.set(task.id, key);
    try {
      await api<AutomationRun>(`automations/${task.id}/runs`, {
        method: "POST",
        body: JSON.stringify({ request_id: key }),
      });
      runKeys.current.delete(task.id);
      setNotice(`Queued ${task.config.name}.`);
      await refresh();
      setSelectedId(task.id);
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy("");
    }
  }
  async function enabled(task: Automation) {
    setBusy(task.id);
    setError("");
    try {
      const updated = await api<Automation>(`automations/${task.id}/enabled`, {
        method: "POST",
        body: JSON.stringify({ enabled: !task.enabled, expected_revision: task.revision }),
      });
      setTasks((previous) => previous.map((entry) => (entry.id === task.id ? updated : entry)));
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy("");
    }
  }
  async function remove() {
    if (!removing) return;
    setBusy(removing.id);
    setError("");
    try {
      await api(`automations/${removing.id}?revision=${removing.revision}`, { method: "DELETE" });
      setTasks((previous) => previous.filter((task) => task.id !== removing.id));
      setRemoving(undefined);
      setSelectedId(undefined);
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy("");
    }
  }
  async function stop(run: AutomationRun) {
    setBusy(run.id);
    setHistoryError("");
    try {
      const stopped = await api<AutomationRun>(`automation-runs/${run.id}/cancel`, {
        method: "POST",
        body: "{}",
      });
      setRuns((previous) => mergeRuns(previous, [stopped]));
      await refresh();
    } catch (failure) {
      setHistoryError((failure as Error).message);
    } finally {
      setBusy("");
    }
  }
  async function loadMore() {
    if (!selected || !cursor || historyBusy) return;
    setHistoryBusy(true);
    try {
      const page = await api<RunPage>(`automations/${selected.id}/runs?cursor=${cursor}`);
      if (selectedRef.current !== selected.id) return;
      setRuns((previous) => mergeRuns(previous, page.runs));
      setCursor(page.next_cursor);
    } catch (failure) {
      if (selectedRef.current === selected.id) setHistoryError((failure as Error).message);
    } finally {
      if (selectedRef.current === selected.id) setHistoryBusy(false);
    }
  }
  const visible = tasks.filter(
    (task) =>
      (filter === "all" ||
        (filter === "enabled"
          ? task.enabled
          : filter === "paused"
            ? !task.enabled
            : task.has_unfinished_run)) &&
      `${task.config.name} ${task.config.prompt}`
        .toLocaleLowerCase()
        .includes(query.trim().toLocaleLowerCase()),
  );
  return (
    <section className="flex h-full min-h-0 flex-col" aria-label="Automations">
      <header className="shrink-0 px-5 pt-6 sm:px-10 sm:pt-8">
        <div className="mx-auto max-w-6xl">
          <div className="flex items-start justify-between gap-4">
            <div>
              <h1 className="text-2xl font-semibold tracking-tight">Automations</h1>
              <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
                Schedule work for your agent. Review each result in its own conversation.
              </p>
            </div>
            <Button
              type="button"
              disabled={!options || loading}
              onClick={() => {
                setError("");
                setEditing(null);
              }}
            >
              <PlusIcon />
              New task
            </Button>
          </div>
          <div className="mt-6 flex items-center justify-between gap-3 border-b pb-4 text-xs text-muted-foreground">
            <span>
              {tasks.length} tasks · {tasks.filter((task) => task.enabled).length} enabled
            </span>
            <span className="inline-flex items-center gap-1.5">
              <span
                className={`size-1.5 rounded-full ${online ? "bg-emerald-500" : "bg-muted-foreground"}`}
              />
              {online ? "Scheduler running" : "Scheduler unavailable"}
            </span>
          </div>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-6 sm:px-10">
        <div className="mx-auto max-w-6xl space-y-5">
          {optionsError && (
            <p role="alert" className="text-sm text-destructive">
              Task settings could not load: {optionsError}. Reopen Automations to retry.
            </p>
          )}
          <div className="flex flex-wrap gap-3">
            <div className="relative min-w-0 flex-1 basis-52">
              <SearchIcon
                aria-hidden
                className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
              />
              <Input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                aria-label="Search automations"
                placeholder="Search tasks…"
                className="h-9 pl-9"
              />
            </div>
            <select
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
              aria-label="Filter automations"
              className="h-9 rounded-lg border bg-background px-3 text-xs"
            >
              <option value="all">All tasks</option>
              <option value="enabled">Enabled</option>
              <option value="paused">Paused</option>
              <option value="running">Unfinished runs</option>
            </select>
            <Button
              type="button"
              variant="outline"
              disabled={refreshing}
              aria-label="Refresh automations"
              onClick={() => void refresh()}
            >
              <RefreshCwIcon className={refreshing ? "animate-spin" : ""} />
            </Button>
          </div>
          {error && !removing && (
            <p
              role="alert"
              className="rounded-xl border border-destructive/20 bg-destructive/5 p-4 text-sm text-destructive"
            >
              {error}
            </p>
          )}
          {notice && !error && (
            <p
              role="status"
              className="rounded-xl border bg-muted/30 p-4 text-sm text-muted-foreground"
            >
              {notice}
            </p>
          )}
          {loading ? (
            <div
              role="status"
              aria-label="Loading automations"
              className="grid gap-4 lg:grid-cols-2"
            >
              {[0, 1, 2, 3].map((item) => (
                <Skeleton key={item} className="h-60 rounded-2xl" />
              ))}
            </div>
          ) : !visible.length ? (
            <div className="flex min-h-80 flex-col items-center justify-center rounded-2xl border border-dashed px-6 py-12 text-center">
              <div className="mb-4 rounded-2xl bg-muted/60 p-4">
                <CalendarClockIcon className="size-6 text-muted-foreground" />
              </div>
              <h2 className="font-medium">
                {query || filter !== "all"
                  ? "No matching tasks"
                  : error
                    ? "Automations could not load"
                    : "Let your agent take care of the routine"}
              </h2>
              <p className="mt-2 max-w-sm text-sm leading-relaxed text-muted-foreground">
                {query || filter !== "all"
                  ? "Try another search or clear the filters."
                  : "Create a schedule for project reviews, reports, or recurring maintenance."}
              </p>
              <Button
                type="button"
                variant="outline"
                className="mt-5"
                disabled={!options}
                onClick={() =>
                  query || filter !== "all" ? (setQuery(""), setFilter("all")) : setEditing(null)
                }
              >
                {query || filter !== "all" ? "Clear filters" : "Create your first task"}
              </Button>
            </div>
          ) : (
            <div className="grid gap-4 lg:grid-cols-2">
              {visible.map((task) => (
                <article
                  key={task.id}
                  className="flex min-w-0 flex-col rounded-2xl border bg-card p-5"
                >
                  <div className="flex items-start gap-3">
                    <div className="rounded-xl border bg-muted/40 p-2.5">
                      <CalendarClockIcon className="size-5 text-muted-foreground" />
                    </div>
                    <div className="min-w-0 flex-1">
                      <h2>
                        <button
                          type="button"
                          onClick={() => setSelectedId(task.id)}
                          className="max-w-full truncate text-left text-sm font-semibold hover:underline"
                        >
                          {task.config.name}
                        </button>
                      </h2>
                      <span
                        className={`mt-1.5 inline-flex items-center gap-1.5 text-[11px] ${task.enabled ? "text-muted-foreground" : "text-muted-foreground/70"}`}
                      >
                        <span
                          className={`size-1.5 rounded-full ${task.enabled ? "bg-emerald-500" : "bg-muted-foreground/50"}`}
                        />
                        {task.enabled ? "Enabled" : "Paused"}
                      </span>
                    </div>
                    <Menu.Root>
                      <Menu.Trigger
                        render={
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            aria-label={`Actions for ${task.config.name}`}
                            disabled={!!busy}
                          />
                        }
                      >
                        <EllipsisIcon />
                      </Menu.Trigger>
                      <Menu.Portal>
                        <Menu.Positioner side="bottom" align="end" sideOffset={5} className="z-50">
                          <Menu.Popup className="min-w-40 rounded-xl border bg-popover p-1 shadow-lg">
                            <Menu.Item className={menuItem} onClick={() => setSelectedId(task.id)}>
                              Run history
                            </Menu.Item>
                            <Menu.Item
                              className={menuItem}
                              disabled={!options}
                              onClick={() => setEditing(task)}
                            >
                              <Settings2Icon className="size-3.5" />
                              Edit
                            </Menu.Item>
                            <Menu.Item className={menuItem} onClick={() => void enabled(task)}>
                              {task.enabled ? (
                                <PauseIcon className="size-3.5" />
                              ) : (
                                <PlayIcon className="size-3.5" />
                              )}
                              {task.enabled ? "Pause schedule" : "Enable schedule"}
                            </Menu.Item>
                            <div role="separator" className="my-1 border-t" />
                            <Menu.Item
                              className={`${menuItem} text-destructive`}
                              disabled={task.has_unfinished_run}
                              onClick={() => {
                                setError("");
                                setRemoving(task);
                              }}
                            >
                              <Trash2Icon className="size-3.5" />
                              Delete
                            </Menu.Item>
                          </Menu.Popup>
                        </Menu.Positioner>
                      </Menu.Portal>
                    </Menu.Root>
                  </div>
                  <p className="mb-4 mt-4 line-clamp-2 min-h-10 text-sm leading-5 text-muted-foreground">
                    {task.config.prompt}
                  </p>
                  <div className="mb-5 space-y-2 text-xs text-muted-foreground">
                    <p className="flex items-center gap-2">
                      <ClockIcon className="size-3.5 shrink-0" />
                      {scheduleLabel(task.config.cron)} · {task.config.timezone}
                    </p>
                    <p className="flex items-center gap-2" title={task.config.cwd}>
                      <FolderIcon className="size-3.5 shrink-0" />
                      <span className="truncate">{workspaceLabel(task.config.cwd)}</span> ·{" "}
                      {task.config.workspace_mode === "project" ? "Project" : "Isolated"}
                    </p>
                    <p>
                      Next run:{" "}
                      {task.enabled ? taskTime(task.next_run_at, task.config.timezone) : "Paused"}
                    </p>
                  </div>
                  <div className="mt-auto flex flex-wrap items-center justify-between gap-3 border-t pt-4">
                    <button
                      type="button"
                      onClick={() => setSelectedId(task.id)}
                      className="text-left text-xs text-muted-foreground hover:text-foreground"
                    >
                      {task.latest_run ? (
                        <>
                          <RunBadge
                            status={task.latest_run.status}
                            stopping={
                              task.latest_run.cancel_requested && unfinished(task.latest_run.status)
                            }
                          />
                          <span className="ml-2 hidden xl:inline">
                            {taskTime(task.latest_run.created_at, task.config.timezone)}
                          </span>
                        </>
                      ) : (
                        "No runs yet"
                      )}
                    </button>
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={!!busy || !online || task.has_unfinished_run}
                      onClick={() => void runNow(task)}
                    >
                      {busy === task.id ? (
                        <LoaderCircleIcon className="animate-spin" />
                      ) : (
                        <PlayIcon />
                      )}
                      Run now
                    </Button>
                  </div>
                </article>
              ))}
            </div>
          )}
        </div>
      </div>
      {editing !== undefined && options && (
        <AutomationForm
          key={editing?.id ?? "new"}
          task={editing ?? undefined}
          options={options}
          onClose={() => setEditing(undefined)}
          onSaved={(task) => {
            setTasks((previous) => [task, ...previous.filter((entry) => entry.id !== task.id)]);
            setEditing(undefined);
            setNotice(`Saved ${task.config.name}.`);
          }}
        />
      )}
      <Sheet
        open={!!selected}
        onOpenChange={(open) => {
          if (!open) setSelectedId(undefined);
        }}
      >
        <SheetContent className="gap-0 data-[side=right]:w-full data-[side=right]:sm:max-w-2xl">
          <div className="shrink-0 border-b p-6 pr-12">
            <SheetTitle className="break-words text-xl font-semibold">
              {selected?.config.name}
            </SheetTitle>
            <SheetDescription className="mt-2">
              {selected && `${scheduleLabel(selected.config.cron)} · ${selected.config.timezone}`}
            </SheetDescription>
            {selected && (
              <div className="mt-4 flex flex-wrap gap-2">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={!!busy || !options}
                  onClick={() => {
                    setSelectedId(undefined);
                    setEditing(selected);
                  }}
                >
                  <Settings2Icon />
                  Edit
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={!!busy}
                  onClick={() => void enabled(selected)}
                >
                  {selected.enabled ? <PauseIcon /> : <PlayIcon />}
                  {selected.enabled ? "Pause" : "Enable"}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  disabled={!!busy || !online || selected.has_unfinished_run}
                  onClick={() => void runNow(selected)}
                >
                  <PlayIcon />
                  Run now
                </Button>
              </div>
            )}
          </div>
          <div className="min-h-0 flex-1 space-y-6 overflow-y-auto p-6">
            {selected && (
              <dl className="space-y-3 rounded-xl border bg-muted/20 p-4 text-xs">
                {[
                  [
                    "Next run",
                    selected.enabled
                      ? taskTime(selected.next_run_at, selected.config.timezone)
                      : "Paused",
                  ],
                  ["Workspace", selected.config.cwd],
                  [
                    "Directory",
                    selected.config.workspace_mode === "project"
                      ? "Project files"
                      : "Private directory per run",
                  ],
                  ["Model", selected.config.model_profile ?? "Service default"],
                  ["Thinking", selected.config.thinking],
                  ["Permissions", selected.config.permission.replaceAll("_", " ")],
                ].map(([label, value]) => (
                  <div key={label} className="grid grid-cols-[90px_1fr] gap-4">
                    <dt className="text-muted-foreground">{label}</dt>
                    <dd className="break-all">{value}</dd>
                  </div>
                ))}
              </dl>
            )}
            <details className="rounded-xl border p-4">
              <summary className="cursor-pointer text-sm font-medium">Task instructions</summary>
              <p className="mt-3 whitespace-pre-wrap break-words text-sm leading-7">
                {selected?.config.prompt}
              </p>
            </details>
            <section>
              <h3 className="mb-4 text-sm font-semibold">Run history</h3>
              {historyError && (
                <p role="alert" className="mb-4 text-sm text-destructive">
                  {historyError}
                </p>
              )}
              {!runs.length ? (
                <p className="rounded-xl border border-dashed p-6 text-sm text-muted-foreground">
                  {historyBusy
                    ? "Loading runs…"
                    : "No runs yet. Each execution will have its own conversation."}
                </p>
              ) : (
                <ol className="space-y-3">
                  {runs.map((run) => (
                    <li key={run.id} className="rounded-xl border p-4">
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <time className="text-xs font-medium">
                          {taskTime(run.scheduled_for, run.snapshot.timezone)}
                        </time>
                        <RunBadge
                          status={run.status}
                          stopping={run.cancel_requested && unfinished(run.status)}
                        />
                      </div>
                      <p className="mt-2 text-[10px] text-muted-foreground">
                        {run.trigger === "manual" ? "Manual run" : "Scheduled run"} · Revision{" "}
                        {run.task_revision}
                        {run.finished_at && run.started_at
                          ? ` · ${Math.max(0, (run.finished_at - run.started_at) / 1000).toFixed(1)}s elapsed`
                          : ""}
                      </p>
                      {(run.summary || run.error) && (
                        <p
                          className={`mt-3 whitespace-pre-wrap break-words text-xs leading-6 ${run.error ? "text-destructive" : "text-muted-foreground"}`}
                        >
                          {run.error ?? run.summary}
                        </p>
                      )}
                      <div className="mt-3 flex gap-2">
                        {run.core_run_id && (
                          <Button
                            type="button"
                            size="sm"
                            variant="outline"
                            onClick={() => {
                              setSelectedId(undefined);
                              onOpenSession(run.session_id);
                            }}
                          >
                            Open conversation
                            <ArrowUpRightIcon />
                          </Button>
                        )}
                        {unfinished(run.status) && (
                          <Button
                            type="button"
                            size="sm"
                            variant="ghost"
                            disabled={!!busy || run.cancel_requested}
                            onClick={() => void stop(run)}
                          >
                            <SquareIcon />
                            {run.cancel_requested ? "Stopping…" : "Stop run"}
                          </Button>
                        )}
                      </div>
                    </li>
                  ))}
                </ol>
              )}
              {cursor && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="mt-4"
                  disabled={historyBusy}
                  onClick={() => void loadMore()}
                >
                  Load more runs
                </Button>
              )}
            </section>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              Missed times and overlaps are recorded as skipped. Failures are not automatically
              retried. Pausing affects future triggers; stop an active run separately.
            </p>
          </div>
        </SheetContent>
      </Sheet>
      <Dialog
        open={!!removing}
        onOpenChange={(open) => {
          if (!open && !busy) setRemoving(undefined);
        }}
      >
        <DialogContent>
          <DialogTitle>Delete {removing?.config.name}?</DialogTitle>
          <DialogDescription>
            This removes the schedule. Existing run history and conversations are retained.
          </DialogDescription>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={!!busy}
              onClick={() => setRemoving(undefined)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={!!busy}
              onClick={() => void remove()}
            >
              Delete task
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </section>
  );
}
