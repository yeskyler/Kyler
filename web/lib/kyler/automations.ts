import type { ModelCatalog, PermissionMode } from "./types.ts";
export type TaskConfig = {
  name: string;
  prompt: string;
  cwd: string;
  workspace_mode: "isolated" | "project";
  cron: string;
  timezone: string;
  model_profile: string | null;
  thinking: "off" | "low" | "high" | "max";
  permission: PermissionMode;
  timeout_seconds: number;
  wait_timeout_seconds: number;
};
export type RunStatus =
  | "queued"
  | "starting"
  | "running"
  | "waiting_for_approval"
  | "waiting_for_input"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted"
  | "skipped";
export type AutomationRun = {
  id: string;
  task_id: string;
  task_revision: number;
  snapshot: TaskConfig;
  trigger: string;
  scheduled_for: number;
  status: RunStatus;
  session_id: string;
  core_run_id: string | null;
  execution_id: string | null;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  cancel_requested: boolean;
  timed_out: boolean;
  summary: string | null;
  error: string | null;
};
export type Automation = {
  id: string;
  revision: number;
  config: TaskConfig;
  enabled: boolean;
  deleted: boolean;
  next_run_at: number | null;
  created_at: number;
  updated_at: number;
  latest_run: AutomationRun | null;
  has_unfinished_run: boolean;
};
export type RunPage = { runs: AutomationRun[]; next_cursor: string | null };
export type AutomationOptions = {
  models: ModelCatalog;
  workspaces: { path: string; title: string }[];
  defaultWorkspace: string;
};
export const runLabels: Record<RunStatus, string> = {
  queued: "Queued",
  starting: "Starting",
  running: "Running",
  waiting_for_approval: "Waiting for approval",
  waiting_for_input: "Waiting for input",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
  interrupted: "Interrupted",
  skipped: "Skipped",
};
export function unfinished(status: RunStatus | undefined) {
  return (
    !!status &&
    ["queued", "starting", "running", "waiting_for_approval", "waiting_for_input"].includes(status)
  );
}
export function taskTime(timestamp: number | null | undefined, timezone: string) {
  if (timestamp == null) return "—";
  try {
    return new Intl.DateTimeFormat(undefined, {
      timeZone: timezone,
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(timestamp);
  } catch {
    return "Invalid time zone";
  }
}
export function scheduleLabel(cron: string) {
  if (cron === "0 * * * *") return "Every hour";
  const parts = cron.trim().split(/\s+/);
  if (
    parts.length === 5 &&
    /^\d+$/.test(parts[0]) &&
    /^\d+$/.test(parts[1]) &&
    parts[2] === "*" &&
    parts[3] === "*"
  ) {
    const time = `${parts[1].padStart(2, "0")}:${parts[0].padStart(2, "0")}`;
    if (parts[4] === "*") return `Daily at ${time}`;
    if (["MON-FRI", "1-5"].includes(parts[4])) return `Weekdays at ${time}`;
    if (["MON", "1"].includes(parts[4])) return `Mondays at ${time}`;
  }
  return cron;
}
export function workspaceLabel(path: string) {
  return (
    path
      .replace(/[\\/]$/, " ")
      .trim()
      .split(/[\\/]/)
      .pop() || path
  );
}
export function mergeRuns(previous: AutomationRun[], incoming: AutomationRun[]) {
  return [...new Map([...previous, ...incoming].map((run) => [run.id, run])).values()].sort(
    (a, b) => b.created_at - a.created_at || b.id.localeCompare(a.id),
  );
}
