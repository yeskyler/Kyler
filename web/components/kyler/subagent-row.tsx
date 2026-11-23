"use client";

import { useAuiState } from "@assistant-ui/react";
import {
  CircleCheckIcon,
  CircleMinusIcon,
  CircleXIcon,
  ClockIcon,
  LoaderCircleIcon,
  ChevronRightIcon,
} from "lucide-react";
import type { SubagentPresentation } from "@/lib/kyler/subagents";

const states = {
  queued: { label: "Queued", Icon: ClockIcon },
  running: { label: "Running", Icon: LoaderCircleIcon },
  completed: { label: "Completed", Icon: CircleCheckIcon },
  failed: { label: "Failed", Icon: CircleXIcon },
  timed_out: { label: "Timed out", Icon: CircleXIcon },
  cancelled: { label: "Cancelled", Icon: CircleMinusIcon },
  interrupted: { label: "Interrupted", Icon: CircleMinusIcon },
};

export function SubagentRow({ task }: { task: SubagentPresentation }) {
  const message = useAuiState((state) => state.message.status);
  const status =
    (task.status === "queued" || task.status === "running") && message?.type !== "running"
      ? message?.type === "incomplete" && message.reason === "cancelled"
        ? "cancelled"
        : "interrupted"
      : task.status;
  const { label, Icon } = states[status] ?? states.interrupted;
  const role = { review: "Code review", memory: "Memory extraction", general: "Subagent" }[
    task.agent
  ];
  const target = task.target;
  const scope =
    target?.type === "uncommitted"
      ? "Uncommitted changes"
      : target?.type === "base_branch"
        ? `Compare with ${target.branch}`
        : target?.type === "commit"
          ? `Commit ${target.commit}`
          : task.agent === "memory"
            ? "Current session history"
            : undefined;
  return (
    <details className="group min-w-0 text-sm text-muted-foreground">
      <summary className="flex min-h-8 cursor-pointer list-none items-center gap-2 rounded-md px-1.5 py-1 text-left hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50 [&::-webkit-details-marker]:hidden">
        <Icon
          aria-hidden
          className={`size-3.5 shrink-0 ${status === "running" ? "animate-spin motion-reduce:animate-none" : ""} ${status === "failed" || status === "timed_out" ? "text-destructive" : ""}`}
        />
        <span className="truncate font-medium text-foreground">
          {task.name === "kyler" ? role : task.name || role}
        </span>
        {task.name && task.name !== "kyler" && <span className="truncate text-xs">{role}</span>}
        <span className="ml-auto shrink-0 text-xs">{label}</span>
        <ChevronRightIcon aria-hidden className="size-3.5 shrink-0 group-open:rotate-90" />
      </summary>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 px-7 pb-2 text-xs leading-5">
        {scope && (
          <>
            <dt>Scope</dt>
            <dd className="break-words text-foreground">{scope}</dd>
          </>
        )}
        {task.summary && (
          <>
            <dt>Result</dt>
            <dd className="whitespace-pre-wrap break-words text-foreground">{task.summary}</dd>
          </>
        )}
        {task.error && (
          <>
            <dt>Reason</dt>
            <dd className="whitespace-pre-wrap break-words text-destructive">{task.error}</dd>
          </>
        )}
        {!scope && !task.summary && !task.error && (
          <>
            <dt>Status</dt>
            <dd>{label}</dd>
          </>
        )}
      </dl>
    </details>
  );
}
