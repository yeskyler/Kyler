"use client";

import { useId, useState } from "react";
import { ChevronDownIcon, ShieldQuestionIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { Approval } from "@/lib/kyler/events";

export function ApprovalCard({
  approval,
  count,
  busy,
  error,
  onDecision,
}: {
  approval: Approval;
  count: number;
  busy: boolean;
  error: string;
  onDecision: (decision: "allow_once" | "deny") => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const detailsId = useId();
  return (
    <section
      aria-label="Tool approval"
      aria-busy={busy}
      className="max-h-[32dvh] w-full shrink-0 overflow-y-auto rounded-xl border border-border bg-muted/30 p-3 text-xs shadow-sm"
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h2 className="flex shrink-0 items-center gap-1.5 font-medium">
          <ShieldQuestionIcon aria-hidden className="size-3.5 text-muted-foreground" />
          Permission required
        </h2>
        <span className="min-w-0 flex-1 break-all font-mono font-medium">{approval.tool}</span>
        {count > 1 && <span className="shrink-0 text-muted-foreground">1 / {count}</span>}
      </div>
      <p className="mt-1.5 break-words leading-5 text-muted-foreground">{approval.reason}</p>
      {error && (
        <p role="alert" className="mt-1.5 text-destructive">
          {error}
        </p>
      )}
      <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={expanded ? detailsId : undefined}
          onClick={() => setExpanded((value) => !value)}
          className="flex h-6 items-center gap-1 rounded text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
        >
          <ChevronDownIcon
            aria-hidden
            className={`size-3 transition-transform ${expanded ? "rotate-180" : ""}`}
          />
          Details
        </button>
        <div className="ml-auto flex gap-1.5">
          <Button size="xs" variant="outline" disabled={busy} onClick={() => onDecision("deny")}>
            Deny
          </Button>
          <Button size="xs" disabled={busy} onClick={() => onDecision("allow_once")}>
            {busy ? "Submitting…" : "Allow once"}
          </Button>
        </div>
      </div>
      {expanded && (
        <pre
          id={detailsId}
          className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-background/70 p-2 leading-5"
        >
          {approval.preview}
        </pre>
      )}
    </section>
  );
}
