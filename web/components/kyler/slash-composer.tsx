"use client";

import { createContext, useContext, useId, useState, type ComponentProps } from "react";
import { useAui, useAuiState } from "@assistant-ui/react";
import { PasteComposerInput } from "./paste-composer";
import {
  commandDraft,
  matchingCommands,
  opensCommandMenu,
  commandRequest,
  type CommandName,
} from "@/lib/kyler/session-commands";

export const SessionCommandContext = createContext<{
  subagents?: boolean;
}>({});

export function SlashComposerInput(props: ComponentProps<typeof PasteComposerInput>) {
  const aui = useAui();
  const { subagents } = useContext(SessionCommandContext);
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
  const [selection, setSelection] = useState(0);
  const list = useId();
  const matches = open ? matchingCommands(text) : [];
  const visible = matches.length > 0;
  const selected = Math.min(selection, matches.length - 1);

  function unavailable(command: CommandName) {
    if (subagents === undefined) return "This server does not support session commands.";
    if (busy) return "Wait for the current task to finish.";
    if (command !== "compact" && !subagents) return "Subagents are disabled on this server.";
    if (command !== "review" && !hasHistory) return "This session has no conversation history yet.";
    return undefined;
  }

  function choose(name: CommandName) {
    const thread = aui.thread();
    if (unavailable(name) || thread.getState().isRunning) return;
    setOpen(false);
    // Append the operation separately. The composer still owns its attachments and skills.
    if (aui.composer().getState().text === text)
      aui.composer().setText(commandDraft(text)?.prefix ?? "");
    thread.append({
      role: "user",
      content: [{ type: "text", text: `/${name}` }],
      runConfig: { custom: { sessionCommand: commandRequest(name) } },
    });
  }

  return (
    <div className="relative w-full">
      {visible && (
        <div
          id={list}
          role="listbox"
          aria-label="Session commands"
          className="absolute inset-x-0 bottom-full z-30 mb-2 rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg"
        >
          {matches.map(({ name, description }, index) => {
            const reason = unavailable(name);
            return (
              <button
                key={name}
                id={`${list}-${name}`}
                type="button"
                role="option"
                tabIndex={-1}
                aria-selected={index === selected}
                aria-disabled={!!reason}
                onMouseDown={(event) => event.preventDefault()}
                onMouseMove={() => setSelection(index)}
                onClick={() => choose(name)}
                className={`flex w-full items-baseline gap-3 rounded-lg px-3 py-2 text-left text-sm ${index === selected ? "bg-accent text-accent-foreground" : ""}`}
              >
                <span className="min-w-20 font-mono">/{name}</span>
                <span className="text-xs text-muted-foreground">{reason ?? description}</span>
              </button>
            );
          })}
        </div>
      )}
      <PasteComposerInput
        {...props}
        placeholder={subagents === undefined ? props.placeholder : "Type / for commands"}
        aria-autocomplete="list"
        aria-controls={visible ? list : undefined}
        aria-activedescendant={matches[selected] ? `${list}-${matches[selected].name}` : undefined}
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
          if (
            event.defaultPrevented ||
            !open ||
            event.nativeEvent.isComposing ||
            event.keyCode === 229
          )
            return;
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            setOpen(false);
          } else if ((event.key === "ArrowUp" || event.key === "ArrowDown") && matches.length) {
            event.preventDefault();
            const step = event.key === "ArrowUp" ? -1 : 1;
            setSelection((selected + step + matches.length) % matches.length);
          } else if (
            visible &&
            event.key === "Enter" &&
            !event.shiftKey &&
            !event.altKey &&
            !event.ctrlKey &&
            !event.metaKey
          ) {
            event.preventDefault();
            if (matches[selected]) choose(matches[selected].name);
          }
        }}
      />
    </div>
  );
}
