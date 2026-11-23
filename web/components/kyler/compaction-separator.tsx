"use client";
import { useAuiState } from "@assistant-ui/react";
import { CircleCheckIcon, CircleMinusIcon, CircleXIcon, LoaderCircleIcon } from "lucide-react";
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
  const Icon = {
    running: LoaderCircleIcon,
    completed: CircleCheckIcon,
    failed: CircleXIcon,
    cancelled: CircleMinusIcon,
    interrupted: CircleMinusIcon,
  }[status];
  return (
    <div
      role="status"
      aria-live="polite"
      className="my-3 flex items-center gap-2 text-xs font-normal text-muted-foreground"
      data-compaction-status={status}
    >
      <Icon
        aria-hidden
        className={`size-3.5 shrink-0 ${status === "running" ? "animate-spin motion-reduce:animate-none" : ""}`}
      />
      <span>{label}</span>
    </div>
  );
}
