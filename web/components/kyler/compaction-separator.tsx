"use client";
import { useAuiState } from "@assistant-ui/react";
import { LoaderCircleIcon } from "lucide-react";
import type { Compaction } from "@/lib/kyler/compaction";

export function CompactionSeparator({ compaction }: { compaction: Compaction }) {
  const message = useAuiState((state) => state.message.status);
  const status =
    compaction.status === "running" && message && message.type !== "running"
      ? message?.type === "incomplete" && message.reason === "cancelled"
        ? "cancelled"
        : "interrupted"
      : compaction.status;
  const label = {
    running: "Compacting context…",
    completed: "Context compacted",
    failed: "Context compaction failed",
    cancelled: "Context compaction cancelled",
    interrupted: "Context compaction interrupted",
  }[status];
  return (
    <div
      role="status"
      aria-live="polite"
      className="my-5 flex w-full items-center gap-3 text-xs font-normal text-muted-foreground"
      data-compaction-status={status}
    >
      <span aria-hidden className="h-px min-w-4 flex-1 bg-border" />
      {status === "running" && (
        <LoaderCircleIcon
          aria-hidden
          className="size-3.5 shrink-0 animate-spin motion-reduce:animate-none"
        />
      )}
      <span>{label}</span>
      <span aria-hidden className="h-px min-w-4 flex-1 bg-border" />
    </div>
  );
}
