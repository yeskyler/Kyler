import assert from "node:assert/strict";
import test from "node:test";
import type { ChatModelRunResult, ThreadAssistantMessagePart } from "@assistant-ui/react";
import { chatAdapter } from "../lib/kyler/client.ts";
import { historyMessages, type HistoryEvent } from "../lib/kyler/history.ts";
import { executionSections, partitionAssistantParts } from "../lib/kyler/assistant-display.ts";
import type { SubagentPresentation } from "../lib/kyler/subagents.ts";

const atlas = {
  task_id: "review",
  name: "atlas",
  agent: "review",
  status: "running",
  target: { type: "uncommitted" },
} satisfies SubagentPresentation;

test("live child tasks retain separate rows and hide their control tool calls", async (t) => {
  const events = [
    { seq: 1, type: "tool_started", name: "delegate_task", call_id: "delegate" },
    { seq: 2, type: "subagent_updated", task: atlas },
    {
      seq: 3,
      type: "subagent_updated",
      task: { ...atlas, task_id: "memory", name: "nova", agent: "memory" },
    },
    { seq: 4, type: "tool_finished", name: "wait_tasks", call_id: "wait" },
    {
      seq: 5,
      type: "subagent_updated",
      task: { ...atlas, status: "completed", summary: "Reviewed code." },
    },
    { seq: 6, type: "run_completed", output: "Done." },
  ];
  t.mock.method(globalThis, "fetch", async (url: string) =>
    url.endsWith("/runs")
      ? Response.json({ run_id: "r" })
      : new Response(events.map((item) => `data: ${JSON.stringify(item)}\n\n`).join("")),
  );
  let final: ChatModelRunResult | undefined;
  for await (const update of chatAdapter("s", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Review" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    final = update;
  const parts = final?.content ?? [];
  assert.deepEqual(
    parts.map((part) => part.type),
    ["data", "data", "text"],
  );
  assert.deepEqual(
    parts.flatMap((part) => (part.type === "data" ? [[part.data.name, part.data.status]] : [])),
    [
      ["atlas", "completed"],
      ["nova", "running"],
    ],
  );
});

test("history renders child tasks independently of the categorized tool tree", () => {
  const events: HistoryEvent[] = [
    {
      id: "event",
      execution_id: "execution",
      timestamp_ms: 1,
      content: [
        { type: "subagent", task: { ...atlas, status: "failed", error: "Could not read target." } },
        {
          type: "tool_call",
          name: "delegate_task",
          call_id: "delegate",
          arguments: {},
          summary: null,
          presentation: null,
        },
        { type: "message", role: "assistant", text: "Review unavailable." },
      ],
    },
  ];
  const parts = historyMessages(events, [], "s")[0].content as ThreadAssistantMessagePart[];
  assert.deepEqual(
    parts.map((part) => part.type),
    ["data", "text"],
  );
  assert.deepEqual(executionSections(parts, partitionAssistantParts(parts).process), [
    { kind: "subagent", indices: [0] },
  ]);
});
