"use client";

import { useId, useMemo, useState, type ReactNode } from "react";
import { useAuiState } from "@assistant-ui/react";
import { CheckIcon, CircleIcon, ListTodoIcon, LoaderCircleIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useChatSettings } from "./chat-settings-context";
import { ComposerPatchSummary, PatchFiles } from "./patch-changes";
import { latestComposerPatch } from "@/lib/kyler/patch-changes";
import { activeDisplayPlan } from "@/lib/kyler/assistant-display";

export function ComposerPlan({ children }: { children: ReactNode }) {
  const { session } = useChatSettings();
  return <ComposerProgress key={session.session_id}>{children}</ComposerProgress>;
}

function ComposerProgress({ children }: { children: ReactNode }) {
  const messages = useAuiState((state) => state.thread.messages);
  const running = useAuiState((state) => state.thread.isRunning);
  const plan = useMemo(() => activeDisplayPlan(messages, running), [messages, running]);
  const latestPatch = useMemo(() => latestComposerPatch(messages), [messages]);
  const [initialPatchKey] = useState(() => latestComposerPatch(messages)?.key);
  const patch = latestPatch?.key === initialPatchKey ? undefined : latestPatch;
  const [panel, setPanel] = useState<string>();
  const open = panel === "plan";
  const patchOpen = !!patch && panel === patch.key;
  const id = useId();
  const patchId = `${id}-patch`;
  const completed = plan?.steps.filter((step) => step.status === "completed").length ?? 0;
  return (
    <section
      aria-label="Composer tools"
      onKeyDown={(event) => {
        if (event.key === "Escape" && (open || patchOpen)) {
          event.preventDefault();
          event.stopPropagation();
          setPanel(undefined);
        }
      }}
    >
      <div className="grid min-h-11 grid-cols-[minmax(0,1fr)_minmax(0,2fr)_minmax(0,1fr)] items-center gap-2 px-3 py-1 text-xs text-muted-foreground">
        <div className="min-w-0 overflow-hidden">{children}</div>
        <div className="min-w-0 text-center" role="status" aria-live="polite">
          {patch && (
            <button
              type="button"
              className="flex w-full min-w-0 items-center justify-center rounded-md px-1 py-1.5 hover:bg-foreground/5"
              title={
                patch.result.files.length === 1
                  ? patch.result.files[0].path
                  : `${patch.result.files.length} changed files`
              }
              aria-label={patchOpen ? "Hide changed files" : "Show changed files"}
              aria-expanded={patchOpen}
              aria-controls={patchId}
              onClick={() => setPanel(patchOpen ? undefined : patch.key)}
            >
              <ComposerPatchSummary result={patch.result} />
            </button>
          )}
        </div>
        {plan && plan.steps.length > 0 && (
          <Button
            variant="ghost"
            size="sm"
            className="h-7 justify-self-end gap-1.5 rounded-md px-2 text-xs font-normal text-muted-foreground hover:bg-foreground/5"
            aria-label={open ? "Hide plan" : "Show plan"}
            aria-expanded={open}
            aria-controls={id}
            title="Plan"
            onClick={() => setPanel(open ? undefined : "plan")}
          >
            <ListTodoIcon className="size-4" strokeWidth={1.5} />
            <span className="tabular-nums">
              Steps {completed}/{plan.steps.length}
            </span>
          </Button>
        )}
      </div>
      {patch && (
        <div
          id={patchId}
          hidden={!patchOpen}
          className="max-h-[min(20rem,35vh)] overflow-auto px-5 pb-4 pt-1"
          aria-label="Latest patch changes"
        >
          <PatchFiles result={patch.result} />
        </div>
      )}
      {plan && plan.steps.length > 0 && (
        <div id={id} hidden={!open} className="px-5 pb-5 pt-1" aria-label="Task progress">
          <h3 className="mb-2 text-xs font-medium text-muted-foreground">Task progress</h3>
          {plan ? (
            <>
              <p className="mb-3 text-sm leading-6 text-muted-foreground">{plan.summary}</p>
              <ol
                className="max-h-[min(24rem,40vh)] space-y-2.5 overflow-y-auto overscroll-contain pr-1"
                aria-label="Plan steps"
              >
                {plan.steps.map((step, index) => {
                  const Icon =
                    step.status === "completed"
                      ? CheckIcon
                      : step.status === "in_progress"
                        ? LoaderCircleIcon
                        : CircleIcon;
                  return (
                    <li
                      key={`${step.id}-${index}`}
                      className={`flex items-start gap-2.5 text-sm leading-6 ${step.status === "pending" ? "text-muted-foreground" : "text-foreground"}`}
                    >
                      <Icon
                        aria-hidden
                        className={`mt-1 size-4 shrink-0 ${step.status === "completed" ? "text-emerald-600 dark:text-emerald-400" : step.status === "in_progress" ? "animate-spin motion-reduce:animate-none" : "opacity-50"}`}
                      />
                      <span className="sr-only">{step.status.replaceAll("_", " ")}: </span>
                      <span
                        className={`min-w-0 break-words ${step.status === "in_progress" ? "font-medium" : ""}`}
                      >
                        {step.content}
                      </span>
                    </li>
                  );
                })}
              </ol>
            </>
          ) : (
            <p className="py-2 text-sm text-muted-foreground">No plan yet.</p>
          )}
        </div>
      )}
    </section>
  );
}
