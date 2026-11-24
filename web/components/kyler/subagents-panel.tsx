"use client";

import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api } from "@/lib/kyler/client";

type Settings = { enabled: boolean; max_concurrent_tasks: number; max_tasks_per_execution: number };
type Snapshot = {
  configured: Settings;
  active: Settings;
  restart_required: boolean;
  max_concurrent_limit: number;
  max_tasks_limit: number;
};
type Draft = { enabled: boolean; concurrent: string; total: string };
const draftFor = (settings: Settings): Draft => ({
  enabled: settings.enabled,
  concurrent: String(settings.max_concurrent_tasks),
  total: String(settings.max_tasks_per_execution),
});

export function SubagentsPanel() {
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [draft, setDraft] = useState<Draft>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    setError("");
    api<Snapshot>("subagents/settings", { signal: controller.signal })
      .then((value) => {
        if (!controller.signal.aborted) {
          setSnapshot(value);
          setDraft(draftFor(value.configured));
        }
      })
      .catch((error: Error) => {
        if (!controller.signal.aborted) setError(error.message);
      });
    return () => controller.abort();
  }, [reload]);
  const concurrent = Number(draft?.concurrent);
  const total = Number(draft?.total);
  const valid =
    snapshot &&
    Number.isInteger(concurrent) &&
    concurrent >= 1 &&
    concurrent <= snapshot.max_concurrent_limit &&
    Number.isInteger(total) &&
    total >= 1 &&
    total <= snapshot.max_tasks_limit;
  const dirty =
    snapshot &&
    draft &&
    (draft.enabled !== snapshot.configured.enabled ||
      concurrent !== snapshot.configured.max_concurrent_tasks ||
      total !== snapshot.configured.max_tasks_per_execution);
  return (
    <div className="space-y-5">
      {snapshot && draft ? (
        <>
          <section
            className="flex flex-wrap items-center justify-between gap-3 rounded-xl border p-5"
            aria-label="Active subagent status"
          >
            <div>
              <h3 className="font-medium">Current runtime</h3>
              <p className="mt-1 text-sm text-muted-foreground">
                {snapshot.active.enabled
                  ? `Up to ${snapshot.active.max_concurrent_tasks} concurrent tasks and ${snapshot.active.max_tasks_per_execution} tasks per execution.`
                  : "Delegation is currently disabled."}
              </p>
            </div>
            <span className="inline-flex items-center gap-2 rounded-full bg-accent px-3 py-1 text-xs">
              <span
                aria-hidden="true"
                className={`size-2 rounded-full ${snapshot.active.enabled ? "bg-emerald-500" : "bg-muted-foreground/50"}`}
              />
              {snapshot.active.enabled ? "Enabled" : "Disabled"}
            </span>
          </section>
          {snapshot.restart_required && (
            <p
              role="status"
              className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-foreground"
            >
              Restart Kyler to apply the saved settings. Current tasks keep their existing limits.
            </p>
          )}
          <form
            className="space-y-6 rounded-xl border p-5"
            onSubmit={async (event) => {
              event.preventDefault();
              if (!valid || !dirty || busy) return;
              setBusy(true);
              setError("");
              try {
                const value = await api<Snapshot>("subagents/settings", {
                  method: "PUT",
                  body: JSON.stringify({
                    enabled: draft.enabled,
                    max_concurrent_tasks: concurrent,
                    max_tasks_per_execution: total,
                  }),
                });
                setSnapshot(value);
                setDraft(draftFor(value.configured));
              } catch (error) {
                setError((error as Error).message);
              } finally {
                setBusy(false);
              }
            }}
          >
            <label className="flex items-start justify-between gap-4">
              <span>
                <span className="block text-sm font-medium">Enable subagents</span>
                <span className="mt-1 block text-sm text-muted-foreground">
                  Allow Kyler to delegate work to child agents.
                </span>
              </span>
              <input
                type="checkbox"
                role="switch"
                className="mt-1 size-4 accent-primary"
                checked={draft.enabled}
                disabled={busy}
                onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
              />
            </label>
            <label className="block space-y-2">
              <span className="text-sm font-medium">Maximum concurrent tasks</span>
              <Input
                type="number"
                required
                min={1}
                max={snapshot.max_concurrent_limit}
                step={1}
                disabled={busy}
                className="max-w-40"
                value={draft.concurrent}
                onChange={(event) => setDraft({ ...draft, concurrent: event.target.value })}
              />
              <span className="block text-xs text-muted-foreground">
                Tasks that can run at the same time. 1–{snapshot.max_concurrent_limit}.
              </span>
            </label>
            <label className="block space-y-2">
              <span className="text-sm font-medium">Maximum tasks per execution</span>
              <Input
                type="number"
                required
                min={1}
                max={snapshot.max_tasks_limit}
                step={1}
                disabled={busy}
                className="max-w-40"
                value={draft.total}
                onChange={(event) => setDraft({ ...draft, total: event.target.value })}
              />
              <span className="block text-xs text-muted-foreground">
                Total child tasks a parent execution can create, including completed tasks. 1–
                {snapshot.max_tasks_limit}.
              </span>
            </label>
            <p className="text-xs text-muted-foreground">
              Changes take effect after restarting Kyler.
            </p>
            <div className="flex justify-end gap-2">
              <Button
                type="button"
                variant="outline"
                disabled={busy || !dirty}
                onClick={() => {
                  setDraft(draftFor(snapshot.configured));
                  setError("");
                }}
              >
                Reset
              </Button>
              <Button type="submit" disabled={busy || !valid || !dirty}>
                {busy ? "Saving…" : "Save"}
              </Button>
            </div>
          </form>
        </>
      ) : (
        !error && (
          <p role="status" className="text-sm text-muted-foreground">
            Loading subagent settings…
          </p>
        )
      )}
      {error && (
        <div role="alert" className="space-y-2 text-sm text-destructive">
          <p>{error}</p>
          {!snapshot && (
            <Button variant="outline" onClick={() => setReload((value) => value + 1)}>
              Retry
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
