"use client";

import { useCallback, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { BookOpenIcon, PlugIcon, PlusIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { PluginSkills } from "./plugin-skills";
import { PluginConnectors } from "./plugin-connectors";

export function ResourcesPanel() {
  const [tab, setTab] = useState<"skills" | "connectors">("skills");
  const [counts, setCounts] = useState<{ skills: number | null; connectors: number | null }>({
    skills: null,
    connectors: null,
  });
  const [addSignal, setAddSignal] = useState(0);
  const running = useAuiState((state) => state.thread.isRunning);
  const skillCount = useCallback(
    (skills: number) => setCounts((previous) => ({ ...previous, skills })),
    [],
  );
  const connectorCount = useCallback(
    (connectors: number) => setCounts((previous) => ({ ...previous, connectors })),
    [],
  );
  return (
    <section className="flex h-full min-h-0 flex-col" aria-label="Plugins">
      <header className="shrink-0 px-5 pt-6 sm:px-10 sm:pt-8">
        <div className="mx-auto w-full max-w-6xl">
          <div className="flex items-start justify-between gap-4">
            <div>
              <h1 className="text-2xl font-semibold tracking-tight">Plugins</h1>
              <p className="mt-2 max-w-xl text-sm leading-relaxed text-muted-foreground">
                Give your agent new skills and connect the tools you work with.
              </p>
            </div>
            <Button
              type="button"
              disabled={running}
              className="mt-0.5"
              onClick={() => setAddSignal((value) => value + 1)}
            >
              <PlusIcon />
              Add {tab === "skills" ? "skill" : "connector"}
            </Button>
          </div>
          <div role="tablist" aria-label="Plugin type" className="mt-7 flex gap-6 border-b">
            {(["skills", "connectors"] as const).map((value) => {
              const Icon = value === "skills" ? BookOpenIcon : PlugIcon;
              return (
                <button
                  key={value}
                  type="button"
                  role="tab"
                  tabIndex={tab === value ? 0 : -1}
                  id={`tab-${value}`}
                  aria-controls={`resource-${value}`}
                  aria-selected={tab === value}
                  onClick={() => setTab(value)}
                  onKeyDown={(event) => {
                    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
                    event.preventDefault();
                    const next =
                      event.key === "Home"
                        ? "skills"
                        : event.key === "End"
                          ? "connectors"
                          : tab === "skills"
                            ? "connectors"
                            : "skills";
                    setTab(next);
                    document.getElementById(`tab-${next}`)?.focus();
                  }}
                  className={`-mb-px flex items-center gap-2 border-b-2 px-0.5 pb-3 text-sm font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50 ${tab === value ? "border-foreground text-foreground" : "border-transparent text-muted-foreground hover:text-foreground"}`}
                >
                  <Icon className="size-4" />
                  {value === "skills" ? "Skills" : "Connectors"}
                  <span className="rounded-md bg-muted px-1.5 py-0.5 text-[10px] tabular-nums text-muted-foreground">
                    {counts[value] ?? "—"}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-6 sm:px-10">
        <div className="mx-auto w-full max-w-6xl">
          <div
            role="tabpanel"
            id="resource-skills"
            aria-labelledby="tab-skills"
            hidden={tab !== "skills"}
          >
            <PluginSkills active={tab === "skills"} addSignal={addSignal} onCount={skillCount} />
          </div>
          <div
            role="tabpanel"
            id="resource-connectors"
            aria-labelledby="tab-connectors"
            hidden={tab !== "connectors"}
          >
            <PluginConnectors
              active={tab === "connectors"}
              addSignal={addSignal}
              onCount={connectorCount}
            />
          </div>
        </div>
      </div>
    </section>
  );
}
