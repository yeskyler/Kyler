import { upsertCompaction, settleCompactions } from "./compaction.ts";
import type { QuestionRecord } from "./questions.ts";
import type {
  AttachmentAdapter,
  ChatModelAdapter,
  ChatModelRunResult,
  ThreadAssistantMessagePart,
} from "@assistant-ui/react";
import { readEvents, type Approval } from "./events.ts";

export class KylerApiError extends Error {
  readonly status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = "KylerApiError";
    this.status = status;
  }
}

export function redirectToLogin() {
  if (typeof window !== "undefined" && window.location.pathname !== "/login") {
    window.location.assign(
      `/login?next=${encodeURIComponent(window.location.pathname + window.location.search + window.location.hash)}`,
    );
  }
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api/kyler/${path}`, {
    ...init,
    headers: {
      ...(typeof init?.body === "string" ? { "content-type": "application/json" } : {}),
      ...init?.headers,
    },
    cache: "no-store",
  });
  if (!response.ok) {
    if (response.status === 401) redirectToLogin();
    const error = await response.json().catch(() => ({}));
    throw new KylerApiError(
      error.message ?? error.error?.message ?? `Kyler request failed (${response.status}).`,
      response.status,
    );
  }
  if (response.status === 204 || response.status === 205) return undefined as T;
  const body = await response.text();
  if (!body.trim()) return undefined as T;
  try {
    return JSON.parse(body) as T;
  } catch {
    throw new Error(`Kyler returned invalid JSON (${response.status}).`);
  }
}

export type SessionReference = string | (() => Promise<{ session_id: string }>);
const resolveSession = async (reference: SessionReference) =>
  typeof reference === "string" ? reference : (await reference()).session_id;

export function attachments(reference: SessionReference): AttachmentAdapter {
  return {
    accept:
      "image/png,image/jpeg,image/webp,image/gif,.txt,.md,.csv,.json,.pdf,.docx,.xlsx,.rs,.py,.js,.ts,.tsx,.toml,.yaml,.yml",
    async add({ file }) {
      const session = await resolveSession(reference);
      if (file.size > 20 * 1024 * 1024) throw new Error("Attachments must be at most 20 MB.");
      const info = await api<{ id: string }>(
        `sessions/${session}/attachments?filename=${encodeURIComponent(file.name)}`,
        { method: "POST", body: file },
      );
      return {
        id: info.id,
        name: file.name,
        file,
        contentType: file.type,
        type: file.type.startsWith("image/") ? "image" : "document",
        status: { type: "requires-action", reason: "composer-send" },
      };
    },
    async send(attachment) {
      return { ...attachment, status: { type: "complete" }, content: [] };
    },
    async remove(attachment) {
      const session = await resolveSession(reference);
      // Completed attachments remain available to the conversation.
      if (attachment.status.type !== "complete")
        await api(`sessions/${session}/attachments/${attachment.id}`, { method: "DELETE" });
    },
  };
}

async function* followRun(run: string, signal: AbortSignal, onReconnect?: () => void) {
  let cursor = 0;
  let retries = 0;
  while (true) {
    signal.throwIfAborted();
    try {
      const response = await fetch(`/api/kyler/runs/${run}/events`, {
        headers: { "last-event-id": String(cursor) },
        signal,
        cache: "no-store",
      });
      if (!response.ok) {
        if (response.status === 401) redirectToLogin();
        throw new KylerApiError(`Cannot read run events (${response.status}).`, response.status);
      }
      if (retries > 0) onReconnect?.();
      for await (const event of readEvents(response)) {
        if (event.seq <= cursor) continue;
        cursor = event.seq;
        retries = 0;
        yield event;
        if (["run_completed", "run_failed", "run_cancelled"].includes(event.type)) return;
      }
    } catch (error) {
      if (signal.aborted || (error instanceof KylerApiError && error.status === 401)) throw error;
      if (retries >= 3) throw error;
    }
    if (++retries > 3)
      throw new Error("The event stream disconnected. Reload to recover the conversation.");
    await new Promise<void>((resolve, reject) => {
      const stop = () => {
        clearTimeout(timer);
        reject(signal.reason);
      };
      const timer = setTimeout(() => {
        signal.removeEventListener("abort", stop);
        resolve();
      }, 300 * retries);
      signal.addEventListener("abort", stop, { once: true });
      if (signal.aborted) stop();
    });
  }
}

export const selectedSessionKey = "kyler.web.session";
export const pendingSessionKey = "kyler.web.pending-session";
export const runStorageKey = (session: string) => `kyler.web.run.${session}`;

export function chatAdapter(
  reference: SessionReference,
  onApproval: (approval: Approval) => void,
  recoverRun?: string,
  lifecycle?: {
    onAdmitted?: (session: string) => void;
    onResponse?: (session: string) => void;
    onQuestion?: (question: QuestionRecord) => void;
    onReconnect?: () => void;
  },
): ChatModelAdapter {
  let recovery = recoverRun;
  return {
    async *run({ messages, abortSignal }) {
      const session = await resolveSession(reference);
      const last = messages.at(-1);
      if (!recovery && last?.role !== "user")
        throw new Error(
          "Only new user turns are supported. Start a new conversation to retry a different prompt.",
        );
      const input =
        last?.content
          .filter((part) => part.type === "text")
          .map((part) => part.text)
          .join("\n") ?? "";
      // Do not abort admission: an accepted run must be known so Stop can cancel it.
      const accepted = recovery
        ? { run_id: recovery }
        : await api<{ run_id: string }>(`sessions/${session}/runs`, {
            method: "POST",
            body: JSON.stringify({
              input,
              attachment_ids: last?.role === "user" ? last.attachments.map((item) => item.id) : [],
              client_request_id: crypto.randomUUID(),
            }),
          });
      recovery = undefined;
      let executionId: string | undefined;
      let startedAt = Date.now();
      let finishedAt: number | undefined;
      let terminal = false;
      let responded = false;
      let step: number | undefined;
      let batch: number | undefined;
      const content: ThreadAssistantMessagePart[] = [];
      const detailsController = new AbortController();
      const detailsSignal = AbortSignal.any([abortSignal, detailsController.signal]);
      let latestPlan: string | undefined;
      const approvalStates = new Map<string, string>();
      let closeEvents: (() => Promise<unknown>) | undefined;
      const hydrations = new Map<string, Promise<{ id: string; apply: () => void }>>();
      const settleTools = (cancelled: boolean) => {
        settleCompactions(content, cancelled ? "cancelled" : "interrupted");
        content.forEach((part, index) => {
          if (part.type !== "tool-call" || part.result !== undefined) return;
          content[index] = {
            ...part,
            result: cancelled ? "Cancelled." : "Outcome unknown; no tool result was received.",
            isError: !cancelled,
            providerMetadata: {
              ...part.providerMetadata,
              kyler: {
                ...part.providerMetadata?.kyler,
                state: cancelled ? "cancelled" : "interrupted",
              },
            },
          };
        });
      };
      const cancel = async () => {
        await api(`runs/${accepted.run_id}/cancel`, { method: "POST", body: "{}" });
      };
      const update = (): ChatModelRunResult => ({
        content: content.map((part) => ({ ...part })),
        metadata: {
          custom: {
            ...(executionId ? { executionId } : {}),
            kylerRun: { startedAt, finishedAt },
          },
        },
      });
      try {
        lifecycle?.onAdmitted?.(session);
        if (typeof localStorage !== "undefined")
          localStorage.setItem(
            runStorageKey(session),
            JSON.stringify({ id: accepted.run_id, input }),
          );
        const events = followRun(accepted.run_id, detailsSignal, lifecycle?.onReconnect);
        closeEvents = () => events.return(undefined);
        let nextEvent: ReturnType<typeof events.next> | undefined;
        while (true) {
          nextEvent ??= events.next();
          const ready = await Promise.race([
            ...[...hydrations.values()].map((task) =>
              task.then((value) => ({ kind: "details" as const, value })),
            ),
            nextEvent.then((value) => ({ kind: "event" as const, value })),
          ]);
          if (ready.kind === "details") {
            hydrations.delete(ready.value.id);
            ready.value.apply();
            yield update();
            continue;
          }
          nextEvent = undefined;
          if (ready.value.done) break;
          const event = ready.value.value;
          if (event.execution_id) executionId = event.execution_id;
          if (event.type === "run_started") {
            startedAt = event.timestamp_ms ?? startedAt;
            yield update();
          } else if (event.type === "context_compaction" && event.compaction) {
            upsertCompaction(content, event.compaction);
            yield update();
          } else if (event.type === "step_started") {
            batch = event.step;
            yield update();
          } else if (event.type === "reasoning_delta") {
            const text = event.text ?? "";
            if (!text) continue;
            const tail = content.at(-1);
            if (tail?.type === "reasoning" && event.step === step)
              content[content.length - 1] = { ...tail, text: tail.text + text };
            else content.push({ type: "reasoning", text });
            step = event.step;
            yield update();
          } else if (event.type === "text_delta") {
            batch = event.step ?? batch;
            if (!responded && event.text?.trim()) {
              lifecycle?.onResponse?.(session);
              responded = true;
            }
            const tail = content.at(-1);
            if (tail?.type === "text" && event.step === step)
              content[content.length - 1] = {
                type: "text",
                text: tail.text + (event.text ?? ""),
              };
            else content.push({ type: "text", text: event.text ?? "" });
            step = event.step;
            yield update();
          } else if (
            event.type === "tool_call_delta" ||
            event.type === "tool_queued" ||
            event.type === "tool_started"
          ) {
            const existing = content.findIndex(
              (part) => part.type === "tool-call" && part.toolCallId === event.call_id,
            );
            const previous = content[existing];
            const tool: ThreadAssistantMessagePart = {
              type: "tool-call",
              toolCallId: event.call_id!,
              toolName: event.name!,
              args: { activity: event.activity ?? "tool", summary: event.summary ?? "" },
              argsText: JSON.stringify({ summary: event.summary ?? "" }),
              providerMetadata: {
                kyler: {
                  ...(batch !== undefined ? { batch } : {}),
                  ...(previous?.type === "tool-call" ? previous.providerMetadata?.kyler : {}),
                  activity: event.activity ?? "tool",
                  summary: event.summary ?? event.name ?? "Preparing tool call",
                  state:
                    event.type === "tool_call_delta"
                      ? "preparing"
                      : event.type === "tool_queued"
                        ? approvalStates.get(event.call_id!) === "pending"
                          ? "approval"
                          : "queued"
                        : "running",
                  ...(event.type === "tool_started"
                    ? { startedAt: event.timestamp_ms ?? Date.now() }
                    : {}),
                  ...(event.source?.kind === "skill"
                    ? { sourceLabel: `Skill · ${event.source.name ?? ""}` }
                    : event.source?.kind === "mcp"
                      ? { sourceLabel: `MCP · ${event.source.server_id ?? ""}` }
                      : {}),
                },
              },
            };
            if (existing < 0) content.push(tool);
            else if (
              previous?.type === "tool-call" &&
              previous.result === undefined &&
              ["tool_call_delta", "tool_queued", "tool_started"].includes(event.type)
            )
              content[existing] = tool;
            yield update();
          } else if (event.type === "tool_finished") {
            const index = content.findIndex(
              (part) => part.type === "tool-call" && part.toolCallId === event.call_id,
            );
            const part = content[index];
            if (part?.type === "tool-call") {
              content[index] = {
                ...part,
                providerMetadata: {
                  ...part.providerMetadata,
                  kyler: {
                    ...part.providerMetadata?.kyler,
                    state: "finished",
                    finishedAt: event.timestamp_ms ?? Date.now(),
                  },
                },
                result:
                  typeof event.error === "string"
                    ? event.error
                    : (event.error?.message ?? event.status ?? "completed"),
                isError:
                  event.status !== "succeeded" &&
                  event.status !== "success" &&
                  event.status !== "completed",
              };
              // Tool events carry presentation summaries; plan steps live in the saved call.
              if (
                part.toolName === "update_plan" &&
                ["success", "succeeded", "completed"].includes(event.status ?? "")
              ) {
                latestPlan = part.toolCallId;
                const id = part.toolCallId;
                const task = api<{ name: string; status: string; arguments: typeof part.args }>(
                  `sessions/${session}/tool-calls/${encodeURIComponent(id)}`,
                  { signal: AbortSignal.any([detailsSignal, AbortSignal.timeout(10000)]) },
                )
                  .catch(() => null)
                  .then((details) => ({
                    id,
                    apply: () => {
                      if (
                        latestPlan !== id ||
                        details?.name !== "update_plan" ||
                        details.status !== "success"
                      )
                        return;
                      const current = content[index];
                      if (current?.type === "tool-call" && current.toolCallId === id)
                        content[index] = {
                          ...current,
                          args: details.arguments,
                          argsText: JSON.stringify(details.arguments),
                        };
                    },
                  }));
                hydrations.set(id, task);
              }
              yield update();
            }
          } else if (event.type === "question_updated" && event.question) {
            if (!responded && event.question.status === "pending") {
              lifecycle?.onResponse?.(session);
              responded = true;
            }
            lifecycle?.onQuestion?.(event.question);
            yield update();
          } else if (event.type === "approval_updated" && event.approval) {
            const approval = event.approval;
            if (approval.tool_call_id) approvalStates.set(approval.tool_call_id, approval.status);
            const index = content.findIndex(
              (part) => part.type === "tool-call" && part.toolCallId === approval.tool_call_id,
            );
            const part = content[index];
            if (part?.type === "tool-call" && part.result === undefined)
              content[index] = {
                ...part,
                providerMetadata: {
                  ...part.providerMetadata,
                  kyler: {
                    ...part.providerMetadata?.kyler,
                    state: approval.status === "pending" ? "approval" : "queued",
                  },
                },
              };
            onApproval(approval);
            yield update();
          } else if (event.type === "run_completed") {
            finishedAt = event.timestamp_ms ?? Date.now();
            terminal = true;
            const complete = event.output_truncated
              ? await api<{ output: string | null }>(`runs/${accepted.run_id}`, {
                  signal: abortSignal,
                }).catch(() => null)
              : null;
            const output = event.output_truncated ? (complete?.output ?? undefined) : event.output;
            if (!responded && output?.trim()) {
              lifecycle?.onResponse?.(session);
              responded = true;
            }
            const tail = content.at(-1);
            if (output?.trim()) {
              if (
                tail?.type === "text" &&
                tail.text.trim() &&
                output.trim().startsWith(tail.text.trim())
              )
                content[content.length - 1] = { type: "text", text: output };
              else if (tail?.type !== "text" || tail.text !== output)
                content.push({ type: "text", text: output });
            }
            settleTools(false);
            yield { ...update(), status: { type: "complete", reason: "stop" } };
            break;
          } else if (event.type === "run_failed") {
            finishedAt = event.timestamp_ms ?? Date.now();
            terminal = true;
            settleTools(false);
            yield update();
            throw new Error(
              typeof event.error === "string"
                ? event.error
                : (event.error?.message ?? "Kyler run failed."),
            );
          } else if (event.type === "run_cancelled") {
            finishedAt = event.timestamp_ms ?? Date.now();
            terminal = true;
            settleTools(true);
            yield { ...update(), status: { type: "incomplete", reason: "cancelled" } };
            break;
          }
        }
      } catch (error) {
        settleCompactions(content, abortSignal.aborted ? "cancelled" : "interrupted");
        yield update();
        throw error;
      } finally {
        detailsController.abort();
        hydrations.clear();
        await closeEvents?.();
        if (!terminal) await cancel();
        if (typeof localStorage !== "undefined") localStorage.removeItem(runStorageKey(session));
      }
    },
  };
}
