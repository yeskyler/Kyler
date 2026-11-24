"use client";

import { useEffect, useRef, useState } from "react";
import { Popover } from "@base-ui/react/popover";
import { useAui, useAuiState } from "@assistant-ui/react";
import {
  BookOpenIcon,
  CheckIcon,
  LoaderCircleIcon,
  PaperclipIcon,
  PlusIcon,
  SearchIcon,
  XIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useChatSettings } from "./chat-settings-context";
import { api } from "@/lib/kyler/client";
import { composerSkills, selectComposerSkill } from "@/lib/kyler/composer-skills";
import { skillSelected } from "@/lib/kyler/plugin-references";
import { filterSkills, skillCategory, pluginStatuses } from "@/lib/kyler/plugins";
import type { Skill } from "@/lib/kyler/types";

export function ComposerMenu() {
  const aui = useAui();
  const { session } = useChatSettings();
  const text = useAuiState((state) => state.composer.text);
  const selected = useAuiState((state) => state.composer.runConfig.custom?.kylerSkills);
  const running = useAuiState((state) => state.thread.isRunning);
  const accept = useAuiState((state) => state.composer.attachmentAccept);
  const [open, setOpen] = useState(false);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [reload, setReload] = useState(0);
  const files = useRef<HTMLInputElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const options = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    const load = () => {
      setLoading(true);
      api<Skill[]>("plugins/skills", { signal: controller.signal })
        .then((skills) => {
          if (!controller.signal.aborted) {
            setSkills(skills);
          }
        })
        .catch((error: Error) => {
          if (!controller.signal.aborted) setError(error.message);
        })
        .finally(() => {
          if (!controller.signal.aborted) setLoading(false);
        });
    };
    load();
    window.addEventListener("kyler-skills-changed", load);
    return () => {
      controller.abort();
      window.removeEventListener("kyler-skills-changed", load);
    };
  }, [open, reload]);
  const filtered = filterSkills(skills, query, "all", "all").filter(
    (skill) => skill.status !== "shadowed" && skill.status !== "invalid",
  );
  return (
    <div className="relative shrink-0">
      <input
        ref={files}
        type="file"
        multiple
        accept={accept || undefined}
        className="hidden"
        aria-label="Upload attachments"
        onChange={async (event) => {
          const selected = Array.from(event.target.files ?? []);
          event.target.value = "";
          try {
            for (const file of selected) await aui.composer().addAttachment(file);
          } catch (error) {
            setError((error as Error).message);
            setOpen(true);
          }
        }}
      />
      <Popover.Root
        open={open}
        onOpenChange={(value) => {
          setOpen(value);
          if (value) {
            setQuery("");
            setError("");
          }
        }}
      >
        <Popover.Trigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-7 rounded-full text-muted-foreground"
              aria-label="Add files or skills"
              disabled={running}
            />
          }
        >
          <PlusIcon className="size-4" />
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Positioner side="top" align="start" sideOffset={8} className="z-50">
            <Popover.Popup
              initialFocus={search}
              finalFocus={() =>
                document.querySelector<HTMLTextAreaElement>(
                  'textarea[aria-label="Message input"]',
                ) ?? true
              }
              className="w-80 max-w-[calc(100vw-2rem)] rounded-2xl border bg-popover p-2 text-popover-foreground shadow-xl outline-none"
              onKeyDown={(event) => {
                if (event.key === "Enter" && document.activeElement === search.current) {
                  event.preventDefault();
                  options.current
                    ?.querySelector<HTMLButtonElement>("button:not(:disabled)")
                    ?.click();
                  return;
                }
                if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
                const items = Array.from(
                  options.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ??
                    [],
                );
                if (!items.length) return;
                event.preventDefault();
                const current = items.indexOf(document.activeElement as HTMLButtonElement);
                items[
                  current < 0
                    ? event.key === "ArrowDown"
                      ? 0
                      : items.length - 1
                    : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length
                ]?.focus();
              }}
            >
              <Popover.Title className="sr-only">Add files or skills</Popover.Title>
              <Popover.Description className="sr-only">
                Upload files or choose skills for your next message.
              </Popover.Description>
              <button
                type="button"
                disabled={!session.project}
                title={!session.project ? "Choose a workspace to upload files." : undefined}
                className="flex min-h-10 w-full items-center gap-3 rounded-xl px-3 py-2 text-left text-sm outline-none hover:bg-accent focus-visible:bg-accent disabled:opacity-45"
                onClick={() => {
                  files.current?.click();
                  setOpen(false);
                }}
              >
                <PaperclipIcon className="size-4 text-muted-foreground" />
                Upload files
              </button>
              <div className="my-2 border-t" />
              <div className="relative mx-1">
                <SearchIcon className="pointer-events-none absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
                <Input
                  ref={search}
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search skills…"
                  aria-label="Search skills to add"
                  className="h-9 bg-muted/40 pl-9 pr-8 shadow-none"
                />
                {query && (
                  <button
                    type="button"
                    aria-label="Clear skill search"
                    onClick={() => {
                      setQuery("");
                      search.current?.focus();
                    }}
                    className="absolute right-2 top-2.5 text-muted-foreground"
                  >
                    <XIcon className="size-4" />
                  </button>
                )}
              </div>
              <div ref={options} className="mt-2 max-h-[min(22rem,55dvh)] overflow-y-auto">
                {loading ? (
                  <p
                    role="status"
                    className="flex items-center gap-2 p-3 text-xs text-muted-foreground"
                  >
                    <LoaderCircleIcon className="size-3.5 animate-spin" />
                    Loading skills…
                  </p>
                ) : (
                  (["system", "user"] as const).map((category) => {
                    const items = filtered.filter((skill) => skillCategory(skill) === category);
                    return (
                      items.length > 0 && (
                        <section
                          key={category}
                          aria-label={`${category === "system" ? "System" : "User"} skills`}
                        >
                          <h3 className="px-3 pb-1 pt-3 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                            {category === "system" ? "System skills" : "User skills"}
                          </h3>
                          {items.map((skill) => {
                            const checked =
                              composerSkills(selected).some((item) => item.name === skill.name) ||
                              skillSelected(text, skill.name);
                            return (
                              <button
                                key={skill.id}
                                type="button"
                                aria-pressed={checked}
                                disabled={skill.status !== "enabled" || running}
                                title={skill.description}
                                className="flex min-h-10 w-full items-center gap-3 rounded-xl px-3 py-2 text-left text-sm outline-none hover:bg-accent focus-visible:bg-accent disabled:opacity-45"
                                onClick={() => {
                                  try {
                                    selectComposerSkill(aui.composer(), skill, !checked);
                                    setOpen(false);
                                  } catch (error) {
                                    setError((error as Error).message);
                                  }
                                }}
                              >
                                <BookOpenIcon className="size-4 shrink-0 text-muted-foreground" />
                                <span className="min-w-0 flex-1 truncate">
                                  {skill.display_name ?? skill.name}
                                </span>
                                {skill.status !== "enabled" ? (
                                  <span className="text-[10px] text-muted-foreground">
                                    {pluginStatuses[skill.status] ??
                                      skill.status.replaceAll("_", " ")}
                                  </span>
                                ) : (
                                  skill.source === "project" && (
                                    <span className="text-[10px] text-muted-foreground">
                                      Project
                                    </span>
                                  )
                                )}
                                {checked && <CheckIcon className="size-4 shrink-0" aria-hidden />}
                              </button>
                            );
                          })}
                        </section>
                      )
                    );
                  })
                )}
                {!loading && !filtered.length && !error && (
                  <p className="p-3 text-sm text-muted-foreground">
                    {query
                      ? "No matching skills."
                      : "No skills available. Manage skills in Plugins."}
                  </p>
                )}
              </div>
              {error && (
                <div role="alert" className="space-y-2 p-3 text-xs text-destructive">
                  <p>{error}</p>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => {
                      setError("");
                      setReload((value) => value + 1);
                    }}
                  >
                    Retry
                  </Button>
                </div>
              )}
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </div>
  );
}
