import type {
  ThreadMessageLike,
  ThreadAssistantMessagePart,
  CompleteAttachment,
} from "@assistant-ui/react";
import { api, KylerApiError } from "./client.ts";
import type { AttachmentInfo } from "./types.ts";

type Json = null | string | number | boolean | Json[] | { [key: string]: Json };

type Presentation = {
  step: number;
  activity: string;
  source: { kind: string; name?: string; server_id?: string };
  started_at: number | null;
  finished_at: number | null;
};
export type HistoryContent =
  | { type: "message"; role: string; text: string }
  | { type: "reasoning"; text: string }
  | { type: "attachment"; attachment: AttachmentInfo }
  | { type: "image"; name: string }
  | {
      type: "tool_call";
      call_id: string;
      name: string;
      arguments: Json;
      summary: string | null;
      presentation: Presentation | null;
    }
  | {
      type: "tool_result";
      call_id: string;
      name: string;
      status: string;
      output: string;
      output_bytes: number;
      truncated: boolean;
    };
export type HistoryEvent = {
  id: string;
  execution_id: string;
  timestamp_ms: number;
  content: HistoryContent[];
};
export type HistoryExecution = {
  execution_id: string;
  status: string;
  started_at: number;
  finished_at: number | null;
};
type HistoryPage = {
  events: HistoryEvent[];
  executions: HistoryExecution[];
  next_cursor: number | null;
};

export async function loadHistory(session: string, recoverExecution?: string | null) {
  const events: HistoryEvent[] = [];
  const executions = new Map<string, HistoryExecution>();
  let cursor: number | null = 0;
  while (cursor !== null) {
    let page: HistoryPage;
    try {
      page = await api(`sessions/${session}/history?limit=500&cursor=${cursor}`);
    } catch (error) {
      if (cursor === 0 && error instanceof KylerApiError && [404, 405].includes(error.status))
        return loadLegacyHistory(session, recoverExecution);
      throw error;
    }
    events.push(...page.events);
    for (const execution of page.executions) executions.set(execution.execution_id, execution);
    if (page.next_cursor !== null && page.next_cursor <= cursor)
      throw new Error("Invalid history pagination cursor.");
    cursor = page.next_cursor;
  }
  return historyMessages(events, [...executions.values()], session, recoverExecution);
}

/** Reconstruct message parts without flattening tool boundaries or losing intermediate text. */
export function historyMessages(
  events: HistoryEvent[],
  executions: HistoryExecution[],
  session: string,
  recoverExecution?: string | null,
): ThreadMessageLike[] {
  const messages: ThreadMessageLike[] = [];
  const runs = new Map(executions.map((execution) => [execution.execution_id, execution]));
  const calls = new Map<string, { content: ThreadAssistantMessagePart[]; index: number }>();
  let assistant: { execution: string; content: ThreadAssistantMessagePart[] } | undefined;
  const ensureAssistant = (event: HistoryEvent) => {
    if (assistant?.execution === event.execution_id) return assistant.content;
    const execution = runs.get(event.execution_id);
    const content: ThreadAssistantMessagePart[] = [];
    messages.push({
      id: `assistant-${event.execution_id}-${event.id}`,
      role: "assistant",
      content,
      createdAt: new Date(event.timestamp_ms),
      status:
        execution?.status === "completed"
          ? { type: "complete", reason: "stop" }
          : {
              type: "incomplete",
              reason: execution?.status === "cancelled" ? "cancelled" : "error",
            },
      metadata: {
        custom: {
          executionId: event.execution_id,
          historyStatus: execution?.status ?? "unknown",
          kylerRun: {
            startedAt: execution?.started_at,
            finishedAt: execution?.finished_at ?? undefined,
          },
        },
      },
    });
    assistant = { execution: event.execution_id, content };
    return content;
  };
  for (const event of events) {
    const user = event.content.filter((item) => item.type === "message" && item.role === "user");
    const attachments: CompleteAttachment[] = event.content.flatMap((item) => {
      if (item.type !== "attachment") return [];
      const file = item.attachment;
      const image = file.kind === "image";
      return [
        {
          id: file.id,
          name: file.name,
          type: image ? "image" : "document",
          status: { type: "complete" },
          contentType: image ? "image/png" : undefined,
          content: image
            ? [
                {
                  type: "image",
                  image: `/api/kyler/sessions/${session}/attachments/${file.id}/content`,
                  filename: file.name,
                },
              ]
            : [],
        },
      ];
    });
    const legacyImages = event.content.flatMap((item) =>
      item.type === "image" ? [`[Image: ${item.name}]`] : [],
    );
    if (user.length || attachments.length || legacyImages.length) {
      assistant = undefined;
      messages.push({
        id: event.id,
        role: "user",
        content: [
          ...user.flatMap((item) => (item.type === "message" ? [item.text] : [])),
          ...legacyImages,
        ].join("\n"),
        attachments,
        createdAt: new Date(event.timestamp_ms),
        metadata: { custom: { executionId: event.execution_id } },
      });
    }
    if (event.execution_id === recoverExecution) continue;
    for (const item of event.content) {
      if (item.type === "reasoning" && item.text.trim())
        ensureAssistant(event).push({ type: "reasoning", text: item.text });
      if (item.type === "message" && item.role === "assistant")
        ensureAssistant(event).push({ type: "text", text: item.text });
      if (item.type === "tool_call") {
        const content = ensureAssistant(event);
        const metadata = item.presentation;
        const source = metadata?.source;
        const execution = runs.get(event.execution_id);
        const part: ThreadAssistantMessagePart = {
          type: "tool-call",
          toolCallId: item.call_id,
          toolName: item.name,
          args:
            item.arguments && typeof item.arguments === "object" && !Array.isArray(item.arguments)
              ? item.arguments
              : {},
          argsText: JSON.stringify(item.arguments),
          ...(execution?.status !== "running"
            ? {
                result:
                  execution?.status === "cancelled"
                    ? "Cancelled."
                    : "Outcome unknown; no tool result was retained.",
                isError: execution?.status !== "cancelled",
              }
            : {}),
          providerMetadata: {
            kyler: {
              state:
                execution?.status === "running"
                  ? metadata && metadata.started_at === null
                    ? "queued"
                    : "running"
                  : execution?.status === "cancelled"
                    ? "cancelled"
                    : "interrupted",
              activity: metadata?.activity ?? "tool",
              summary: item.summary ?? item.name,
              ...(metadata
                ? {
                    batch: metadata.step,
                    ...(metadata.started_at !== null ? { startedAt: metadata.started_at } : {}),
                    ...(metadata.finished_at !== null ? { finishedAt: metadata.finished_at } : {}),
                  }
                : {}),
              ...(source?.kind === "mcp"
                ? { sourceLabel: `MCP · ${source.server_id ?? ""}` }
                : source?.kind === "skill"
                  ? { sourceLabel: `Skill · ${source.name ?? ""}` }
                  : {}),
            },
          },
        };
        calls.set(`${event.execution_id}:${item.call_id}`, { content, index: content.length });
        content.push(part);
      }
      if (item.type === "tool_result") {
        const target = calls.get(`${event.execution_id}:${item.call_id}`);
        const content = target?.content ?? ensureAssistant(event);
        const index = target?.index ?? content.length;
        const previous = content[index];
        const part =
          previous?.type === "tool-call"
            ? previous
            : {
                type: "tool-call" as const,
                toolCallId: item.call_id,
                toolName: item.name,
                args: {},
                argsText: "{}",
              };
        const cancelled =
          part.providerMetadata?.kyler?.state === "cancelled" &&
          typeof part.providerMetadata?.kyler?.batch === "number" &&
          part.providerMetadata?.kyler?.finishedAt === undefined &&
          item.status !== "success";
        content[index] = {
          ...part,
          result: item.output,
          isError: item.status !== "success" && !cancelled,
          providerMetadata: {
            ...part.providerMetadata,
            kyler: {
              ...part.providerMetadata?.kyler,
              state: cancelled ? "cancelled" : "finished",
              historyOutput: item.output,
              outputTruncated: item.truncated,
              outputBytes: item.output_bytes,
            },
          },
        };
      }
    }
  }
  return messages;
}

async function loadLegacyHistory(session: string, recoverExecution?: string | null) {
  const events: HistoryEvent[] = [];
  let cursor: number | null = 0;
  while (cursor !== null) {
    const page: {
      messages: {
        id: string;
        execution_id: string;
        timestamp: number;
        role: string;
        content: string;
      }[];
      next_cursor: number | null;
    } = await api(`sessions/${session}/messages?limit=500&cursor=${cursor}`);
    events.push(
      ...page.messages.map((message) => ({
        id: message.id,
        execution_id: message.execution_id,
        timestamp_ms: message.timestamp * 1000,
        content: [{ type: "message" as const, role: message.role, text: message.content }],
      })),
    );
    if (page.next_cursor !== null && page.next_cursor <= cursor)
      throw new Error("Invalid history pagination cursor.");
    cursor = page.next_cursor;
  }
  return historyMessages(events, [], session, recoverExecution);
}
