"use client";

import { useEffect, useState } from "react";
import {
  ArrowLeftIcon,
  CheckIcon,
  ChevronRightIcon,
  LoaderCircleIcon,
  ShieldAlertIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { api } from "@/lib/kyler/client";
import { randomUuid } from "@/lib/kyler/random-uuid";
import { permissionLabels } from "@/lib/kyler/types";
import {
  scheduleLabel,
  taskTime,
  workspaceLabel,
  type Automation,
  type AutomationOptions,
  type TaskConfig,
} from "@/lib/kyler/automations";
const selectClass =
  "h-9 w-full rounded-lg border border-input bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50";
export function AutomationForm({
  task,
  options,
  onClose,
  onSaved,
}: {
  task?: Automation;
  options: AutomationOptions;
  onClose: () => void;
  onSaved: (task: Automation) => void;
}) {
  const [config, setConfig] = useState<TaskConfig>(
    () =>
      task?.config ?? {
        name: "",
        prompt: "",
        workspace_root: options.defaultWorkspace,
        workspace_mode: "project",
        cron: "0 9 * * *",
        timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
        model_profile: options.models.default_profile,
        thinking: "high",
        permission: "workspace_access",
        timeout_seconds: 600,
        wait_timeout_seconds: 300,
      },
  );
  const [step, setStep] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [creationId] = useState(randomUuid);
  const [preview, setPreview] = useState<number[]>([]);
  const [previewError, setPreviewError] = useState("");
  const [previewing, setPreviewing] = useState(true);
  const [preset, setPreset] = useState(() =>
    ["0 * * * *", "0 9 * * *", "0 9 * * MON-FRI", "0 9 * * MON"].includes(config.cron)
      ? config.cron
      : "custom",
  );
  const change = <K extends keyof TaskConfig>(key: K, value: TaskConfig[K]) =>
    setConfig((current) => ({ ...current, [key]: value }));
  useEffect(() => {
    const controller = new AbortController();
    setPreviewing(true);
    setPreview([]);
    setPreviewError("");
    const timer = setTimeout(() => {
      void api<{ occurrences: number[] }>("automations/preview", {
        method: "POST",
        body: JSON.stringify({ cron: config.cron, timezone: config.timezone }),
        signal: controller.signal,
      })
        .then((value) => {
          if (!controller.signal.aborted) setPreview(value.occurrences);
        })
        .catch((failure: Error) => {
          if (!controller.signal.aborted) setPreviewError(failure.message);
        })
        .finally(() => {
          if (!controller.signal.aborted) setPreviewing(false);
        });
    }, 350);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [config.cron, config.timezone]);
  async function save(enabled: boolean) {
    if (
      busy ||
      previewing ||
      !preview.length ||
      (config.permission === "full_access" && !confirmed)
    )
      return;
    setBusy(true);
    setError("");
    try {
      const saved = await api<Automation>(task ? `automations/${task.id}` : "automations", {
        method: task ? "PUT" : "POST",
        body: JSON.stringify({
          config,
          enabled,
          expected_revision: task?.revision ?? null,
          request_id: task ? null : creationId,
          confirm_full_access: confirmed,
        }),
      });
      onSaved(saved);
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const roots = [
    ...new Map(
      [
        ...options.workspaces,
        { path: config.workspace_root, title: workspaceLabel(config.workspace_root) },
      ]
        .filter((entry) => entry.path)
        .map((entry) => [entry.path, entry]),
    ).values(),
  ];
  const validContent = !!config.name.trim() && !!config.prompt.trim() && !!config.workspace_root;
  const validExecution =
    Number.isFinite(config.timeout_seconds) &&
    config.timeout_seconds >= 1 &&
    config.timeout_seconds <= 86400 &&
    Number.isFinite(config.wait_timeout_seconds) &&
    config.wait_timeout_seconds >= 1 &&
    config.wait_timeout_seconds <= 86400 &&
    !!config.model_profile &&
    (config.permission !== "full_access" || confirmed);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-2xl">
        <DialogTitle>{task ? "Edit automation" : "New automation"}</DialogTitle>
        <DialogDescription>
          Choose what your agent should do, when to run it, and how it can act.
        </DialogDescription>
        <ol className="mb-2 flex items-center gap-2 text-xs">
          {["Task", "Schedule", "Execution"].map((label, index) => (
            <li
              key={label}
              className={`flex items-center gap-2 ${index === step ? "text-foreground" : "text-muted-foreground"}`}
            >
              <span
                className={`flex size-6 items-center justify-center rounded-full ${index === step ? "bg-foreground text-background" : "bg-muted"}`}
              >
                {index < step ? <CheckIcon className="size-3.5" /> : index + 1}
              </span>
              {label}
              {index < 2 && <ChevronRightIcon className="mx-1 size-3.5 text-muted-foreground" />}
            </li>
          ))}
        </ol>
        <fieldset disabled={busy} className="contents">
          {step === 0 && (
            <div className="space-y-4">
              <label className="block space-y-2 text-sm font-medium">
                <span>Name</span>
                <Input
                  value={config.name}
                  maxLength={100}
                  onChange={(event) => change("name", event.target.value)}
                  placeholder="Daily project review"
                  disabled={busy}
                />
              </label>
              <label className="block space-y-2 text-sm font-medium">
                <span>Instructions</span>
                <Textarea
                  value={config.prompt}
                  maxLength={32000}
                  onChange={(event) => change("prompt", event.target.value)}
                  placeholder="Review the project, summarize recent changes, and save a report…"
                  className="min-h-32"
                  disabled={busy}
                />
              </label>
              <label className="block space-y-2 text-sm font-medium">
                <span>Workspace</span>
                <select
                  value={config.workspace_root}
                  onChange={(event) => change("workspace_root", event.target.value)}
                  className={selectClass}
                >
                  {roots.map((entry) => (
                    <option key={entry.path} value={entry.path}>
                      {entry.title}
                    </option>
                  ))}
                </select>
                <span className="block break-all text-xs font-normal text-muted-foreground">
                  {config.workspace_root || "Add a workspace from the sidebar first."}
                </span>
              </label>
              <fieldset className="space-y-2">
                <legend className="mb-2 text-sm font-medium">Execution directory</legend>
                {(["project", "isolated"] as const).map((mode) => (
                  <label
                    key={mode}
                    className={`flex cursor-pointer items-start gap-3 rounded-xl border p-3 ${config.workspace_mode === mode ? "border-foreground/30 bg-muted/30" : ""}`}
                  >
                    <input
                      type="radio"
                      name="automation-workspace"
                      checked={config.workspace_mode === mode}
                      onChange={() => change("workspace_mode", mode)}
                    />
                    <span>
                      <span className="block text-sm font-medium">
                        {mode === "project" ? "Project directory" : "Isolated directory"}
                      </span>
                      <span className="mt-1 block text-xs text-muted-foreground">
                        {mode === "project"
                          ? "Work directly with the project's existing files."
                          : "Use a new private directory each time. Project files are not automatically copied."}
                      </span>
                    </span>
                  </label>
                ))}
              </fieldset>
            </div>
          )}
          {step === 1 && (
            <div className="space-y-4">
              <label className="block space-y-2 text-sm font-medium">
                <span>Frequency</span>
                <select
                  value={preset}
                  onChange={(event) => {
                    setPreset(event.target.value);
                    if (event.target.value !== "custom") change("cron", event.target.value);
                  }}
                  className={selectClass}
                >
                  <option value="0 * * * *">Every hour</option>
                  <option value="0 9 * * *">Every day at 09:00</option>
                  <option value="0 9 * * MON-FRI">Weekdays at 09:00</option>
                  <option value="0 9 * * MON">Mondays at 09:00</option>
                  <option value="custom">Custom Cron</option>
                </select>
              </label>
              {preset === "custom" && (
                <label className="block space-y-2 text-sm font-medium">
                  <span>Cron expression</span>
                  <Input
                    value={config.cron}
                    onChange={(event) => change("cron", event.target.value)}
                    placeholder="0 9 * * MON-FRI"
                  />
                  <span className="block text-xs font-normal text-muted-foreground">
                    Minute · Hour · Day · Month · Weekday. Minimum interval: one minute.
                  </span>
                </label>
              )}
              <label className="block space-y-2 text-sm font-medium">
                <span>Time zone</span>
                <Input
                  value={config.timezone}
                  onChange={(event) => change("timezone", event.target.value)}
                  placeholder="Asia/Shanghai"
                />
              </label>
              <div className="rounded-xl border bg-muted/20 p-4">
                <p className="text-sm font-medium">Next five scheduled runs</p>
                {previewing ? (
                  <p role="status" className="mt-3 text-xs text-muted-foreground">
                    Checking schedule…
                  </p>
                ) : previewError ? (
                  <p role="alert" className="mt-3 text-xs text-destructive">
                    {previewError}
                  </p>
                ) : (
                  <ol className="mt-3 space-y-2 text-xs tabular-nums text-muted-foreground">
                    {preview.map((time) => (
                      <li key={time}>{taskTime(time, config.timezone)}</li>
                    ))}
                  </ol>
                )}
                <p className="mt-3 text-[11px] text-muted-foreground">
                  {config.timezone} · Missed times are skipped. Overlapping runs are not started.
                </p>
              </div>
            </div>
          )}
          {step === 2 && (
            <div className="space-y-4">
              <div className="grid gap-4 sm:grid-cols-2">
                <label className="space-y-2 text-sm font-medium">
                  <span>Model</span>
                  <select
                    value={config.model_profile ?? ""}
                    onChange={(event) => change("model_profile", event.target.value)}
                    className={selectClass}
                  >
                    <option value="" disabled>
                      Select a model
                    </option>
                    {options.models.models.map((model) => (
                      <option key={model.profile} value={model.profile}>
                        {model.model} · {model.profile}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="space-y-2 text-sm font-medium">
                  <span>Thinking level</span>
                  <select
                    value={config.thinking}
                    onChange={(event) =>
                      change("thinking", event.target.value as TaskConfig["thinking"])
                    }
                    className={selectClass}
                  >
                    {["off", "low", "high", "max"].map((value) => (
                      <option key={value} value={value}>
                        {value[0].toUpperCase() + value.slice(1)}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              <label className="block space-y-2 text-sm font-medium">
                <span>Permissions</span>
                <select
                  value={config.permission}
                  onChange={(event) => {
                    change("permission", event.target.value as TaskConfig["permission"]);
                    setConfirmed(false);
                  }}
                  className={selectClass}
                >
                  {Object.entries(permissionLabels).map(([value, label]) => (
                    <option key={value} value={value}>
                      {label}
                    </option>
                  ))}
                </select>
              </label>
              {config.permission === "full_access" && (
                <div className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-4">
                  <p className="flex items-center gap-2 text-sm font-medium">
                    <ShieldAlertIcon className="size-4" />
                    Full access for this task
                  </p>
                  <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
                    The agent can modify host files, execute commands and perform sensitive
                    operations beyond the workspace without routine approvals. Use only for a task
                    you trust. Operating system permissions still apply.
                  </p>
                  <label className="mt-3 flex items-start gap-2 text-xs">
                    <input
                      type="checkbox"
                      checked={confirmed}
                      onChange={(event) => setConfirmed(event.target.checked)}
                    />
                    I understand and authorize full access for this task revision.
                  </label>
                </div>
              )}
              <div className="grid gap-4 sm:grid-cols-2">
                <label className="space-y-2 text-sm font-medium">
                  <span>Execution limit (seconds)</span>
                  <Input
                    type="number"
                    min={1}
                    max={86400}
                    value={config.timeout_seconds}
                    onChange={(event) => change("timeout_seconds", Number(event.target.value))}
                  />
                </label>
                <label className="space-y-2 text-sm font-medium">
                  <span>Approval / input wait limit (seconds)</span>
                  <Input
                    type="number"
                    min={1}
                    max={86400}
                    value={config.wait_timeout_seconds}
                    onChange={(event) => change("wait_timeout_seconds", Number(event.target.value))}
                  />
                </label>
              </div>
              <p className="text-xs leading-relaxed text-muted-foreground">
                Service limits still apply. Required approvals and questions wait for you in the
                run's linked conversation.
              </p>
              <div className="rounded-xl border bg-muted/20 p-4 text-xs leading-relaxed">
                <p className="font-medium">{config.name}</p>
                <p className="mt-1 text-muted-foreground">
                  {scheduleLabel(config.cron)} · {config.timezone}
                </p>
                <p className="mt-1 break-all text-muted-foreground">
                  {config.workspace_root} ·{" "}
                  {config.workspace_mode === "project"
                    ? "Project directory"
                    : "Private directory per run"}
                </p>
              </div>
            </div>
          )}
        </fieldset>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <div className="mt-2 flex flex-wrap items-center justify-between gap-2 border-t pt-4">
          <Button
            type="button"
            variant="ghost"
            disabled={busy}
            onClick={() => (step === 0 ? onClose() : setStep(step - 1))}
          >
            {step > 0 && <ArrowLeftIcon />}
            {step === 0 ? "Cancel" : "Back"}
          </Button>
          {step < 2 ? (
            <Button
              type="button"
              disabled={
                busy ||
                (step === 0 ? !validContent : previewing || !preview.length || !!previewError)
              }
              onClick={() => setStep(step + 1)}
            >
              Continue
              <ChevronRightIcon />
            </Button>
          ) : (
            <div className="flex gap-2">
              <Button
                type="button"
                variant="outline"
                disabled={busy || !validContent || !validExecution || previewing || !preview.length}
                onClick={() => void save(false)}
              >
                Save paused
              </Button>
              <Button
                type="button"
                disabled={busy || !validContent || !validExecution || previewing || !preview.length}
                onClick={() => void save(task?.enabled ?? true)}
              >
                {busy && <LoaderCircleIcon className="animate-spin" />}
                {task ? "Save changes" : "Save & enable"}
              </Button>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
