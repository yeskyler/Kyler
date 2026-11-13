"use client";

import { useEffect, useId, useRef, useState } from "react";
import {
  ActionBarPrimitive,
  ErrorPrimitive,
  MessagePrimitive,
  useAuiState,
  useScrollLock,
  type ToolCallMessagePartProps,
} from "@assistant-ui/react";
import {
  ClockIcon,
  MinusIcon,
  CheckIcon,
  ChevronDownIcon,
  CopyIcon,
  LoaderCircleIcon,
  XCircleIcon,
  ShieldQuestionIcon,
} from "lucide-react";
import { MessageQuestion, QuestionHistory, usePendingQuestion } from "./questions";
import { api } from "@/lib/kyler/client";
import { useChatSettings } from "./chat-settings-context";
import { ArtifactMarkdown, MessageArtifacts } from "./artifacts";
import { Image } from "@/components/assistant-ui/elements/image";
import { File } from "@/components/assistant-ui/elements/file";
import { Reasoning } from "@/components/assistant-ui/elements/reasoning.aui";
import { ToolFallback } from "@/components/assistant-ui/elements/tool-fallback.aui";
import {
  CollapsibleRoot,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/assistant-ui/elements/collapsible-root";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import {
  formatRunDuration,
  isVisibleMessagePart,
  partitionAssistantParts,
  groupProcessParts,
  groupToolCategories,
} from "@/lib/kyler/assistant-display";

import { executionSections, isCompactionPart, type Compaction } from "@/lib/kyler/compaction";
import { CompactionSeparator } from "./compaction-separator";

function ActivityTool(props: ToolCallMessagePartProps) {
  if (props.toolName === "ask_question")
    return (
      <QuestionHistory callId={props.toolCallId} fallback={<StandardActivityTool {...props} />} />
    );
  return <StandardActivityTool {...props} />;
}
function StandardActivityTool(props: ToolCallMessagePartProps) {
  const [expanded, setExpanded] = useState(false);
  const panelId = useId();
  const data = props.providerMetadata?.kyler;
  const summary = typeof data?.summary === "string" ? data.summary : "";
  const messageStatus = useAuiState((state) => state.message.status);
  const cancelled =
    data?.state === "cancelled" ||
    (props.result === undefined &&
      messageStatus?.type === "incomplete" &&
      messageStatus.reason === "cancelled");
  const preparing = !cancelled && data?.state === "preparing" && props.result === undefined;
  const queued = !cancelled && data?.state === "queued" && props.result === undefined;
  const failed = !cancelled && (props.isError || props.status.type === "incomplete");
  const waiting =
    !cancelled && (data?.state === "approval" || props.status.type === "requires-action");
  const running =
    !preparing &&
    !queued &&
    !cancelled &&
    !failed &&
    !waiting &&
    (data?.state === "running" || props.status.type === "running");
  const StatusIcon = preparing
    ? LoaderCircleIcon
    : queued
      ? ClockIcon
      : cancelled
        ? MinusIcon
        : failed
          ? XCircleIcon
          : waiting
            ? ShieldQuestionIcon
            : running
              ? LoaderCircleIcon
              : CheckIcon;
  const status = preparing
    ? "Preparing tool call"
    : queued
      ? "Queued"
      : cancelled
        ? "Cancelled"
        : failed
          ? "Failed"
          : waiting
            ? "Approval required"
            : running
              ? "Running"
              : "Completed";
  const source = typeof data?.sourceLabel === "string" ? data.sourceLabel : undefined;
  const now = Date.now();
  const elapsed =
    typeof data?.startedAt === "number" && (running || typeof data.finishedAt === "number")
      ? formatRunDuration(
          (typeof data.finishedAt === "number" ? data.finishedAt : now) - data.startedAt,
        )
      : undefined;
  const error =
    failed && typeof props.result === "string"
      ? props.result
          .split("\n")
          .find((line) => line.trim())
          ?.slice(0, 180)
      : undefined;
  return (
    <div className="min-w-0">
      <button
        type="button"
        title={`${props.toolName} · ${summary} · ${status}`}
        aria-label={`${props.toolName}. ${summary}. ${status}. Toggle tool details`}
        aria-expanded={expanded}
        aria-controls={expanded ? panelId : undefined}
        onClick={() => setExpanded((value) => !value)}
        className={`flex min-h-8 w-full min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-left text-xs outline-none transition-colors hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring/50 ${failed ? "text-destructive" : waiting ? "text-amber-600 dark:text-amber-400" : "text-muted-foreground"}`}
      >
        <StatusIcon
          aria-hidden
          className={`size-3.5 shrink-0 ${running || preparing ? "animate-spin motion-reduce:animate-none" : ""}`}
        />
        <span className="max-w-[45%] shrink-0 truncate font-mono font-medium">
          {props.toolName}
        </span>
        {summary && summary !== props.toolName && (
          <span className="min-w-0 flex-1 truncate opacity-75">{summary}</span>
        )}
        {source && (
          <span className="hidden max-w-32 shrink-0 truncate rounded bg-muted/60 px-1.5 text-[10px] sm:inline">
            {source}
          </span>
        )}
        {elapsed && (
          <span className="ml-auto shrink-0 text-[10px] tabular-nums opacity-60">{elapsed}</span>
        )}
        <ChevronDownIcon
          aria-hidden
          className={`size-3 shrink-0 opacity-50 transition-transform ${expanded ? "rotate-180" : ""}`}
        />
      </button>
      {error && !expanded && (
        <p className="mb-1 ml-7 truncate text-xs text-destructive" title={error}>
          {error}
        </p>
      )}
      {expanded && (
        <div
          id={panelId}
          role="region"
          aria-label={`${props.toolName} details`}
          className="mb-2 ml-7 mt-1 rounded-lg border border-border/50 bg-muted/20 p-3"
        >
          <ActivityDetails {...props} />
        </div>
      )}
    </div>
  );
}

function ActivityDetails(props: ToolCallMessagePartProps) {
  const { session } = useChatSettings();
  const [details, setDetails] = useState<{
    arguments: unknown;
    output: string;
    status: string;
    truncated?: boolean;
  }>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  useEffect(() => {
    if (
      props.status.type === "requires-action" ||
      ["preparing", "queued", "approval"].includes(String(props.providerMetadata?.kyler?.state))
    ) {
      setLoading(false);
      return;
    }
    const retained = props.providerMetadata?.kyler;
    if (typeof retained?.historyOutput === "string") {
      setDetails({
        arguments: props.args,
        output: retained.historyOutput,
        status: props.isError ? "error" : "success",
        truncated: retained.outputTruncated === true,
      });
      setLoading(false);
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    setError("");
    api<NonNullable<typeof details>>(
      `sessions/${session.session_id}/tool-calls/${encodeURIComponent(props.toolCallId)}`,
      { signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) },
    )
      .then((value) => {
        if (!controller.signal.aborted) setDetails(value);
      })
      .catch(() => {
        if (!controller.signal.aborted)
          setError("Tool details could not load. Close and reopen this node to retry.");
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [
    session.session_id,
    props.toolCallId,
    props.result,
    props.status.type,
    props.args,
    props.isError,
    props.providerMetadata,
  ]);
  if (props.status.type === "requires-action") return <ToolFallback {...props} />;
  const output = details?.status === "pending" ? undefined : details?.output;
  return (
    <div className="space-y-3 text-xs leading-5">
      <p className="break-all font-mono font-medium">{props.toolName}</p>
      {loading && (
        <p role="status" className="text-muted-foreground">
          Loading tool details…
        </p>
      )}
      {error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
      <div>
        <p className="mb-1 text-muted-foreground">Arguments</p>
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-words font-mono">
          {details ? JSON.stringify(details.arguments, null, 2) : props.argsText || "{}"}
        </pre>
      </div>
      <div>
        <p className="mb-1 text-muted-foreground">Output</p>
        <pre
          className={`max-h-72 overflow-auto whitespace-pre-wrap break-words font-mono ${props.isError ? "text-destructive" : ""}`}
        >
          {output ||
            (props.status.type === "running" || details?.status === "pending"
              ? "Waiting for output…"
              : details
                ? "No output."
                : "Output not loaded.")}
        </pre>
      </div>
      {details?.truncated && <p className="text-muted-foreground">Output preview truncated.</p>}
    </div>
  );
}

function ActivityBranch({ category, indices }: { category: string; indices: number[] }) {
  const [open, setOpen] = useState(true);
  const panelId = useId();
  return (
    <div className="min-w-0">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        className="flex min-h-7 items-center gap-2 rounded-md px-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <span className="font-medium">{category}</span>
        <span className="text-[11px] opacity-55">
          · {indices.length} {indices.length === 1 ? "call" : "calls"}
        </span>
        <ChevronDownIcon
          aria-hidden
          className={`size-3 opacity-50 transition-transform ${open ? "" : "-rotate-90"}`}
        />
      </button>
      {open && (
        <ul id={panelId} className="ml-2.5 list-none p-0">
          {indices.map((index) => (
            <li
              key={index}
              className="relative min-w-0 pl-5 before:absolute before:inset-y-0 before:left-0 before:border-l before:border-border/70 after:absolute after:left-0 after:top-4 after:w-3 after:border-t after:border-border/70 last:before:bottom-auto last:before:h-4"
            >
              <MessagePrimitive.PartByIndex index={index} components={components} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ActivityTree({ indices }: { indices: number[] }) {
  const message = useAuiState((state) => state.message);
  if (message.role !== "assistant") return null;
  const branches = groupToolCategories(message.content, indices);
  return (
    <div role="group" aria-label={`${indices.length} tool calls`} className="min-w-0 space-y-2">
      {branches.map((branch) => (
        <ActivityBranch key={branch.indices[0]} {...branch} />
      ))}
    </div>
  );
}

function ThinkingBlock({ index }: { index: number }) {
  const message = useAuiState((state) => state.message);
  if (message.role !== "assistant") return null;
  const part = message.content[index];
  if (part?.type !== "reasoning" || !part.text.trim()) return null;
  const streaming = message.status?.type === "running" && index === message.content.length - 1;
  return (
    <Reasoning.Root streaming={streaming} variant="ghost" className="mb-0">
      <Reasoning.Trigger active={streaming} />
      <Reasoning.Content aria-busy={streaming}>
        <Reasoning.Text className="max-h-48 overflow-y-auto text-sm text-muted-foreground">
          <MessagePrimitive.PartByIndex index={index} components={components} />
        </Reasoning.Text>
      </Reasoning.Content>
    </Reasoning.Root>
  );
}

const components = {
  Text: ArtifactMarkdown,
  Reasoning,
  Image,
  File,
  tools: { Fallback: ActivityTool },
};

export function AssistantMessage() {
  const pendingQuestion = usePendingQuestion();
  const message = useAuiState((state) => state.message);
  const running = message.status?.type === "running";
  const [disclosure, setDisclosure] = useState({ running, open: running });
  if (disclosure.running !== running) setDisclosure({ running, open: running });
  const open = disclosure.running === running ? disclosure.open : running;
  const processRoot = useRef<HTMLDivElement>(null);
  const lockScroll = useScrollLock(processRoot, 200);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);
  if (message.role !== "assistant") return null;
  const { process, answer } = partitionAssistantParts(message.content);
  const sections = executionSections(message.content, process);
  const details = process.filter((index) => !isCompactionPart(message.content[index]));
  const toolCount = message.content.filter(
    (part) => part.type === "tool-call" && isVisibleMessagePart(part),
  ).length;
  const failedTools = message.content.filter(
    (part) => part.type === "tool-call" && part.isError && isVisibleMessagePart(part),
  ).length;
  const timing = message.metadata.custom.kylerRun as
    | { startedAt?: number; finishedAt?: number }
    | undefined;
  const elapsed =
    !running && timing?.startedAt
      ? formatRunDuration((timing.finishedAt ?? now) - timing.startedAt)
      : undefined;
  const failed = message.status?.type === "incomplete";
  const historyStatus = message.metadata.custom.historyStatus;
  const label = running
    ? pendingQuestion
      ? "Waiting for input"
      : "Working"
    : historyStatus === "running"
      ? "Working"
      : historyStatus === "unknown"
        ? "History"
        : failed
          ? message.status.reason === "cancelled"
            ? "Stopped"
            : "Interrupted"
          : "Completed";
  return (
    <MessagePrimitive.Root
      className="min-w-0 px-2 text-base leading-7 text-foreground"
      data-role="assistant"
    >
      <CollapsibleRoot
        ref={processRoot}
        open={open}
        onOpenChange={(value) => {
          lockScroll();
          setDisclosure({ running, open: value });
        }}
        className="mb-5"
      >
        <CollapsibleTrigger
          disabled={!details.length}
          className="flex w-full items-center gap-2 border-b border-border/65 pb-3 text-left text-sm font-normal text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 disabled:cursor-default disabled:hover:text-muted-foreground"
          aria-label={`${label}${elapsed ? `, ${elapsed}` : ""}. ${details.length ? "Toggle execution details" : "No execution details"}`}
        >
          {running && !pendingQuestion && (
            <LoaderCircleIcon
              aria-hidden
              className="size-3.5 animate-spin motion-reduce:animate-none"
            />
          )}
          <span>
            {label}
            {elapsed ? ` · ${elapsed}` : ""}
            {toolCount > 0 ? ` · ${toolCount} ${toolCount === 1 ? "tool" : "tools"}` : ""}
          </span>
          {failedTools > 0 && (
            <span className="text-destructive">
              · {failedTools} {failedTools === 1 ? "tool failed" : "tools failed"}
            </span>
          )}
          {!!details.length && label !== "Working" && (
            <ChevronDownIcon
              aria-hidden
              className={`size-3.5 transition-transform ${open ? "rotate-180" : ""}`}
            />
          )}
        </CollapsibleTrigger>
        {sections.map((section) => {
          const index = section.indices[0];
          const part = message.content[index];
          if (section.kind === "compaction" && part.type === "data")
            return (
              <CompactionSeparator key={part.id ?? index} compaction={part.data as Compaction} />
            );
          const rows = groupProcessParts(message.content, section.indices);
          return (
            <CollapsibleContent key={index}>
              <div className="space-y-4 pt-4" aria-label="Execution details">
                {rows.map((row) =>
                  row.kind === "tool" ? (
                    <ActivityTree key={row.indices[0]} indices={row.indices} />
                  ) : message.content[row.indices[0]]?.type === "reasoning" ? (
                    <ThinkingBlock key={row.indices[0]} index={row.indices[0]} />
                  ) : (
                    <MessagePrimitive.PartByIndex
                      key={row.indices[0]}
                      index={row.indices[0]}
                      components={components}
                    />
                  ),
                )}
              </div>
            </CollapsibleContent>
          );
        })}
      </CollapsibleRoot>
      <div className="space-y-4" aria-label="Assistant response">
        {answer.map((index) => (
          <MessagePrimitive.PartByIndex key={index} index={index} components={components} />
        ))}
      </div>
      <MessageQuestion />
      <MessagePrimitive.Error>
        <ErrorPrimitive.Root
          role="alert"
          className="mt-4 rounded-lg border border-destructive/20 bg-destructive/5 px-3 py-2 text-sm text-destructive"
        >
          <ErrorPrimitive.Message />
        </ErrorPrimitive.Root>
      </MessagePrimitive.Error>
      <MessageArtifacts />
      <ActionBarPrimitive.Root
        hideWhenRunning={false}
        autohide="never"
        className="mt-2 flex text-muted-foreground"
      >
        <ActionBarPrimitive.Copy render={<TooltipIconButton tooltip="Copy response" />}>
          {message.isCopied ? <CheckIcon /> : <CopyIcon />}
        </ActionBarPrimitive.Copy>
      </ActionBarPrimitive.Root>
    </MessagePrimitive.Root>
  );
}
