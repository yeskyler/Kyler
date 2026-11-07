"use client";

import { useId, useMemo, useState, type ReactNode } from "react";
import { useAuiState } from "@assistant-ui/react";
import { CheckIcon, CircleIcon, ListTodoIcon, LoaderCircleIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { latestDisplayPlan } from "@/lib/kyler/assistant-display";

export function ComposerPlan({ children }: { children: ReactNode }) {
  const messages = useAuiState((state) => state.thread.messages);
  const plan = useMemo(() => latestDisplayPlan(messages), [messages]);
  const [open, setOpen] = useState(false);
  const id = useId();
  const completed = plan?.steps.filter((step) => step.status === "completed").length ?? 0;
  return (
    <section
      aria-label="Composer tools"
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.preventDefault();
          event.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <div className="flex min-h-11 items-center justify-between gap-3 px-3 py-1 text-xs text-muted-foreground">
        {children}
        {plan && plan.steps.length > 0 && (
          <Button
            variant="ghost"
            size="sm"
            className="h-7 gap-1.5 rounded-md px-2 text-xs font-normal text-muted-foreground hover:bg-foreground/5"
            aria-label={open ? "Hide plan" : "Show plan"}
            aria-expanded={open}
            aria-controls={id}
            title="Plan"
            onClick={() => setOpen(!open)}
          >
            <ListTodoIcon className="size-4" strokeWidth={1.5} />
            <span className="tabular-nums">
              Steps {completed}/{plan.steps.length}
            </span>
          </Button>
        )}
      </div>
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
