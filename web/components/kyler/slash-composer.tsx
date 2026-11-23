"use client";

import { createContext, useContext, useId, useRef, useState, type ComponentProps } from "react";
import { useAui, useAuiState } from "@assistant-ui/react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { PasteComposerInput } from "./paste-composer";
import {
  commandDraft,
  matchingCommands,
  opensCommandMenu,
  reviewTarget,
  type CommandName,
  type ReviewTarget,
  type SessionCommand,
} from "@/lib/kyler/session-commands";

export const SessionCommandContext = createContext<{
  subagents?: boolean;
  hasWorkspace: boolean;
}>({ hasWorkspace: false });

export function SlashComposerInput(props: ComponentProps<typeof PasteComposerInput>) {
  const aui = useAui();
  const { subagents, hasWorkspace } = useContext(SessionCommandContext);
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
  const [reviewOpen, setReviewOpen] = useState(false);
  const [type, setType] = useState<ReviewTarget["type"]>("uncommitted");
  const [reference, setReference] = useState("");
  const dialogOrigin = useRef("");
  const input = useRef<HTMLTextAreaElement>(null);
  const list = useId();
  const matches = open ? matchingCommands(text) : [];
  const visible = matches.length > 0;
  const selected = Math.min(selection, matches.length - 1);
  const target = reviewTarget(type, reference);

  function unavailable(command: CommandName) {
    if (subagents === undefined) return "This server does not support session commands.";
    if (busy) return "Wait for the current task to finish.";
    if (command !== "compact" && !subagents) return "Subagents are disabled on this server.";
    if (command === "review" && !hasWorkspace) return "Code review requires a workspace.";
    if (command !== "review" && !hasHistory) return "This session has no conversation history yet.";
    return undefined;
  }

  function submit(command: SessionCommand, commandText: string) {
    const thread = aui.thread();
    if (unavailable(command.type) || thread.getState().isRunning) return;
    setOpen(false);
    setReviewOpen(false);
    // Append the operation separately. The composer still owns its attachments and skills.
    if (aui.composer().getState().text === commandText)
      aui.composer().setText(commandDraft(commandText)?.prefix ?? "");
    thread.append({
      role: "user",
      content: [{ type: "text", text: `/${command.type}` }],
      runConfig: { custom: { sessionCommand: command } },
    });
  }

  function choose(command: CommandName) {
    if (unavailable(command)) return;
    if (command === "review") {
      dialogOrigin.current = text;
      setType("uncommitted");
      setReference("");
      setOpen(false);
      setReviewOpen(true);
    } else {
      submit({ type: command }, text);
    }
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
        ref={input}
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
      <Dialog open={reviewOpen} onOpenChange={setReviewOpen}>
        <DialogContent finalFocus={input}>
          <DialogTitle>Review code</DialogTitle>
          <DialogDescription>Choose what to review in this workspace.</DialogDescription>
          <label className="grid gap-2">
            Scope
            <select
              value={type}
              onChange={(event) => setType(event.target.value as ReviewTarget["type"])}
              className="h-9 rounded-md border bg-background px-2"
            >
              <option value="uncommitted">Uncommitted changes</option>
              <option value="base_branch">Compare with a branch</option>
              <option value="commit">Specific commit</option>
            </select>
          </label>
          {type !== "uncommitted" && (
            <label className="grid gap-2">
              {type === "commit" ? "Commit ID" : "Branch name"}
              <input
                value={reference}
                onChange={(event) => setReference(event.target.value)}
                className="h-9 rounded-md border bg-background px-2"
              />
            </label>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setReviewOpen(false)}>
              Cancel
            </Button>
            <Button
              type="button"
              disabled={!target || !!unavailable("review")}
              onClick={() => target && submit({ type: "review", target }, dialogOrigin.current)}
            >
              Start review
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
