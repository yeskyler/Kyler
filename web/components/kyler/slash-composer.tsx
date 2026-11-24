"use client";

import { createContext, useContext, useEffect, useId, useState, type ComponentProps } from "react";
import { useAui, useAuiState } from "@assistant-ui/react";
import { CheckIcon } from "lucide-react";
import { PasteComposerInput } from "./paste-composer";
import {
  commandDraft,
  matchingCommands,
  opensCommandMenu,
  commandRequest,
  skillCommandName,
  commandGroups,
  type SlashCommand,
} from "@/lib/kyler/session-commands";
import { composerSkills, selectComposerSkill } from "@/lib/kyler/composer-skills";
import { removeSkillReference, skillSelected } from "@/lib/kyler/plugin-references";
import { api } from "@/lib/kyler/client";
import type { Skill } from "@/lib/kyler/types";

export const SessionCommandContext = createContext<{ subagents?: boolean }>({});

export function SlashComposerInput(props: ComponentProps<typeof PasteComposerInput>) {
  const aui = useAui();
  const { subagents } = useContext(SessionCommandContext);
  const selections = useAuiState((state) => state.composer.runConfig.custom?.kylerSkills);
  const text = useAuiState((state) => state.composer.text);
  const busy = useAuiState(
    (state) =>
      state.thread.isRunning ||
      state.thread.isSendDisabled ||
      state.composer.submission !== undefined,
  );
  const hasHistory = useAuiState((state) =>
    state.thread.messages.some((message) => message.role === "assistant"),
  );
  const [open, setOpen] = useState(false);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [error, setError] = useState("");
  const [selection, setSelection] = useState(0);
  const list = useId();
  const matches = open ? matchingCommands(text, skills) : [];
  const visible = matches.length > 0;
  const selected = Math.max(0, Math.min(selection, matches.length - 1));

  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    const load = () => {
      setLoading(true);
      api<Skill[]>("plugins/skills", { signal: controller.signal })
        .then((items) => {
          if (!controller.signal.aborted) {
            setSkills(items);
            setError("");
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
  }, [open]);
  useEffect(() => {
    if (open) document.getElementById(`${list}-${selected}`)?.scrollIntoView({ block: "nearest" });
  }, [list, open, selected]);

  function unavailable(command: SlashCommand) {
    if (busy || choosing) return "Wait for the current task to finish.";
    if (command.kind === "skill") return undefined;
    if (subagents === undefined) return "This server does not support session commands.";
    if (command.name === "memory" && !subagents) return "Subagents are disabled on this server.";
    if (!hasHistory) return "This session has no conversation history yet.";
    return undefined;
  }
  async function choose(command: SlashCommand) {
    const thread = aui.thread();
    if (unavailable(command) || thread.getState().isRunning) return;
    setOpen(false);
    setError("");
    if (command.kind === "skill") {
      setChoosing(true);
      try {
        const latest = await api<Skill[]>("plugins/skills");
        setSkills(latest);
        if (
          !latest.some((skill) => skill.name === command.skill.name && skill.status === "enabled")
        )
          throw new Error(`Enable ${command.description} in Plugins before selecting it.`);
        if (aui.composer().getState().text === text) {
          const skill = latest.find(
            (skill) => skill.name === command.skill.name && skill.status === "enabled",
          )!;
          selectComposerSkill(aui.composer(), skill);
          aui
            .composer()
            .setText(
              removeSkillReference(commandDraft(text.trimEnd())?.prefix ?? text, skill.name),
            );
        }
      } catch (error) {
        setError((error as Error).message);
      } finally {
        setChoosing(false);
      }
      return;
    }
    if (aui.composer().getState().text === text)
      aui.composer().setText(commandDraft(text)?.prefix ?? "");
    thread.append({
      role: "user",
      content: [{ type: "text", text: `/${command.name}` }],
      runConfig: { custom: { sessionCommand: commandRequest(command.name) } },
    });
  }
  return (
    <div className="relative w-full">
      {open && (
        <div
          id={list}
          role="listbox"
          aria-label="Commands and skills"
          className="absolute inset-x-0 bottom-full z-30 mb-2 max-h-[min(24rem,60vh)] overflow-y-auto rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg"
        >
          {loading && (
            <p role="status" className="px-3 py-2 text-xs text-muted-foreground">
              Loading skills…
            </p>
          )}
          {commandGroups.map((group) => {
            const items = matches.filter((command) => command.group === group);
            return (
              items.length > 0 && (
                <div key={group} role="group" aria-label={group}>
                  <p className="px-3 pb-1 pt-3 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    {group}
                  </p>
                  {items.map((command) => {
                    const index = matches.indexOf(command);
                    const reason = unavailable(command);
                    const checked =
                      command.kind === "skill" &&
                      (composerSkills(selections).some(
                        (skill) => skill.name === command.skill.name,
                      ) ||
                        skillSelected(text, command.skill.name));
                    return (
                      <button
                        key={`${command.kind}-${command.name}`}
                        id={`${list}-${index}`}
                        type="button"
                        role="option"
                        tabIndex={-1}
                        aria-selected={index === selected}
                        aria-disabled={!!reason}
                        onMouseDown={(event) => event.preventDefault()}
                        onMouseMove={() => setSelection(index)}
                        onClick={() => void choose(command)}
                        className={`flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm ${index === selected ? "bg-accent text-accent-foreground" : ""}`}
                      >
                        <span className="w-36 max-w-[45%] shrink-0 truncate font-mono">
                          /{command.name}
                        </span>
                        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                          {reason ?? command.description}
                        </span>
                        {command.kind === "skill" && command.skill.source === "project" && (
                          <span className="text-[10px] text-muted-foreground">Project</span>
                        )}
                        {checked && <CheckIcon className="size-4 shrink-0" aria-label="Selected" />}
                      </button>
                    );
                  })}
                </div>
              )
            );
          })}
          {!loading && !matches.length && (
            <p className="px-3 py-2 text-sm text-muted-foreground">
              No matching commands or skills.
            </p>
          )}
        </div>
      )}
      {error && (
        <p role="alert" className="mb-2 px-2 text-xs text-destructive">
          {error}
        </p>
      )}
      <PasteComposerInput
        {...props}
        placeholder={
          subagents === undefined ? props.placeholder : "Type a message, or / for commands"
        }
        aria-autocomplete="list"
        aria-controls={open ? list : undefined}
        aria-activedescendant={visible ? `${list}-${selected}` : undefined}
        onBlur={(event) => {
          props.onBlur?.(event);
          setOpen(false);
        }}
        onChange={(event) => {
          props.onChange?.(event);
          const native = event.nativeEvent as InputEvent;
          if (opensCommandMenu(text, event.target.value, native.inputType) && !native.isComposing)
            setOpen(true);
          else if (!commandDraft(event.target.value) || native.inputType === "insertFromPaste")
            setOpen(false);
          setSelection(0);
        }}
        onKeyDown={(event) => {
          props.onKeyDown?.(event);
          if (event.defaultPrevented || event.nativeEvent.isComposing || event.keyCode === 229)
            return;
          if (open && event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            setOpen(false);
            return;
          }
          if (open && (event.key === "ArrowUp" || event.key === "ArrowDown") && matches.length) {
            event.preventDefault();
            const step = event.key === "ArrowUp" ? -1 : 1;
            setSelection((selected + step + matches.length) % matches.length);
            return;
          }
          if (
            (event.key !== "Enter" && event.key !== "Tab") ||
            event.shiftKey ||
            event.altKey ||
            event.ctrlKey ||
            event.metaKey
          )
            return;
          const token = commandDraft(text.trimEnd())?.token.slice(1).toLowerCase();
          const command = open
            ? matches[selected]
            : matchingCommands(text.trimEnd(), skills).find(
                (command) => command.kind === "skill" && command.name === token,
              );
          if (!command) {
            if (
              token === "review" ||
              skills.some((skill) => skillCommandName(skill.name) === token)
            ) {
              event.preventDefault();
              event.stopPropagation();
              setError("This skill is unavailable. Enable it in Plugins before selecting it.");
            }
            return;
          }
          event.preventDefault();
          event.stopPropagation();
          if (event.key === "Tab" && command.kind === "conversation") {
            aui.composer().setText(`${commandDraft(text)?.prefix ?? ""}/${command.name}`);
            setOpen(false);
          } else void choose(command);
        }}
      />
    </div>
  );
}
