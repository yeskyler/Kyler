import assert from "node:assert/strict";
import { test } from "node:test";
import type { ThreadAssistantMessagePart, ChatModelRunResult } from "@assistant-ui/react";
import {
  upsertCompaction,
  settleCompactions,
  executionSections,
  type Compaction,
} from "../lib/kyler/compaction.ts";
import { partitionAssistantParts } from "../lib/kyler/assistant-display.ts";
import { historyMessages, type HistoryEvent } from "../lib/kyler/history.ts";
import { chatAdapter } from "../lib/kyler/client.ts";

const record: Compaction = { id: "c1", status: "running", started_at: 1, finished_at: null };
test("compaction updates in place and remains outside collapsed process sections", () => {
  const parts: ThreadAssistantMessagePart[] = [{ type: "text", text: "Before" }];
  upsertCompaction(parts, record);
  parts.push({ type: "text", text: "After" });
  upsertCompaction(parts, { ...record, status: "completed", finished_at: 2 });
  upsertCompaction(parts, record);
  assert.equal(parts.length, 3);
  assert.equal(parts[1].type === "data" && parts[1].data.status, "completed");
  const { process, answer } = partitionAssistantParts(parts);
  assert.deepEqual(answer, [2]);
  assert.deepEqual(executionSections(parts, process), [
    { kind: "details", indices: [0] },
    { kind: "compaction", indices: [1] },
  ]);
  upsertCompaction(parts, { ...record, id: "c2" });
  settleCompactions(parts, "cancelled");
  assert.equal(parts[1].type === "data" && parts[1].data.status, "completed");
  assert.equal(parts[3].type === "data" && parts[3].data.status, "cancelled");
});

test("history reconstructs one separator and never animates unconfirmed pending records", () => {
  const events: HistoryEvent[] = [
    {
      id: "a",
      execution_id: "e",
      timestamp_ms: 1,
      content: [{ type: "context_compaction", compaction: record }],
    },
    {
      id: "b",
      execution_id: "e",
      timestamp_ms: 2,
      content: [{ type: "message", role: "assistant", text: "Continued" }],
    },
    {
      id: "c",
      execution_id: "e",
      timestamp_ms: 3,
      content: [
        {
          type: "context_compaction",
          compaction: { ...record, status: "completed", finished_at: 3 },
        },
      ],
    },
  ];
  const messages = historyMessages(
    events,
    [{ execution_id: "e", status: "completed", started_at: 0, finished_at: 4 }],
    "s",
  );
  const parts = messages[0].content as ThreadAssistantMessagePart[];
  assert.equal(parts.length, 2);
  assert.equal(parts[0].type === "data" && parts[0].data.status, "completed");
  const interrupted = historyMessages(events.slice(0, 1), [], "s")[0]
    .content as ThreadAssistantMessagePart[];
  assert.equal(interrupted[0].type === "data" && interrupted[0].data.status, "interrupted");
});

test("live SSE updates a separator before final text and settles cancelled compression", async (t) => {
  const events = [
    { seq: 1, type: "context_compaction", compaction: record },
    {
      seq: 2,
      type: "context_compaction",
      compaction: { ...record, status: "completed", finished_at: 2 },
    },
    { seq: 3, type: "text_delta", step: 1, text: "Done" },
    { seq: 4, type: "run_completed", output: "Done" },
  ];
  t.mock.method(globalThis, "fetch", async (url: string) =>
    url.endsWith("/runs")
      ? Response.json({ run_id: "r" })
      : new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("")),
  );
  const updates: ChatModelRunResult[] = [];
  for await (const value of chatAdapter("s", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Continue" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    updates.push(value);
  assert.equal(
    updates[0].content?.[0].type === "data" && updates[0].content[0].data.status,
    "running",
  );
  const final = updates.at(-1)!.content!;
  assert.equal(final.filter((part) => part.type === "data").length, 1);
  assert.equal(final[0].type === "data" && final[0].data.status, "completed");
  events.splice(1, 3, { seq: 2, type: "run_cancelled" } as (typeof events)[number]);
  let cancelled: ChatModelRunResult | undefined;
  for await (const value of chatAdapter("s", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Continue" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    cancelled = value;
  const part = cancelled?.content?.[0];
  assert.equal(part?.type === "data" && part.data.status, "cancelled");
});
