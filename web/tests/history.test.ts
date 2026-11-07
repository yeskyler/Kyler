import assert from "node:assert/strict";
import test from "node:test";
import {
  historyMessages,
  loadHistory,
  type HistoryEvent,
  type HistoryExecution,
} from "../lib/kyler/history.ts";

const execution: HistoryExecution = {
  execution_id: "e1",
  status: "completed",
  started_at: 1000,
  finished_at: 4000,
};
const events: HistoryEvent[] = [
  {
    id: "u",
    execution_id: "e1",
    timestamp_ms: 1000,
    content: [
      { type: "message", role: "user", text: "Inspect" },
      {
        type: "attachment",
        attachment: {
          id: "image",
          name: "screen.png",
          kind: "image",
          bytes: 100,
          sha256: "hash",
          created_at: 1000,
        },
      },
    ],
  },
  {
    id: "a",
    execution_id: "e1",
    timestamp_ms: 2000,
    content: [{ type: "message", role: "assistant", text: "Reading project files." }],
  },
  {
    id: "t",
    execution_id: "e1",
    timestamp_ms: 2000,
    content: [
      {
        type: "tool_call",
        call_id: "c1",
        name: "read_file",
        arguments: { file_path: "main.rs" },
        summary: "main.rs",
        presentation: {
          step: 1,
          activity: "explore",
          source: { kind: "mcp", server_id: "files" },
          started_at: 2100,
          finished_at: 2200,
        },
      },
    ],
  },
  {
    id: "r",
    execution_id: "e1",
    timestamp_ms: 2200,
    content: [
      {
        type: "tool_result",
        call_id: "c1",
        name: "read_file",
        status: "success",
        output: "fn main() {}",
        output_bytes: 12,
        truncated: false,
      },
    ],
  },
  {
    id: "f",
    execution_id: "e1",
    timestamp_ms: 4000,
    content: [{ type: "message", role: "assistant", text: "Verified." }],
  },
];

test("history restores commentary, tool boundaries, retained results, attachments and timing", () => {
  const messages = historyMessages(events, [execution], "session");
  assert.equal(messages.length, 2);
  assert.equal(messages[0].attachments?.[0].name, "screen.png");
  assert.deepEqual(messages[0].attachments?.[0].content, [
    {
      type: "image",
      image: "/api/kyler/sessions/session/attachments/image/content",
      filename: "screen.png",
    },
  ]);
  assert.equal(messages[0].createdAt?.getTime(), 1000);
  const content = messages[1].content;
  assert.notEqual(typeof content, "string");
  if (typeof content === "string") throw new Error("Expected structured content");
  assert.deepEqual(
    content.map((part) => part.type),
    ["text", "tool-call", "text"],
  );
  const call = content[1];
  assert.ok(call.type === "tool-call");
  assert.equal(call.result, "fn main() {}");
  assert.equal(call.isError, false);
  assert.equal(call.providerMetadata?.kyler?.sourceLabel, "MCP · files");
  assert.equal(call.providerMetadata?.kyler?.historyOutput, "fn main() {}");
  assert.deepEqual(messages[1].metadata?.custom?.kylerRun, { startedAt: 1000, finishedAt: 4000 });
  assert.equal(messages[1].status?.type, "complete");
});

test("paginated history joins a tool result to the call from an earlier page", async (t) => {
  const paths: string[] = [];
  t.mock.method(globalThis, "fetch", async (path: string) => {
    paths.push(path);
    return Response.json(
      paths.length === 1
        ? { events: events.slice(0, 3), executions: [execution], next_cursor: 3 }
        : { events: events.slice(3), executions: [execution], next_cursor: null },
    );
  });
  const messages = await loadHistory("session");
  assert.equal(messages.length, 2);
  assert.equal(paths[1], "/api/kyler/sessions/session/history?limit=500&cursor=3");
  assert.deepEqual(messages, historyMessages(events, [execution], "session"));
});

test("active SSE recovery keeps the user turn and attachments without replaying saved assistant parts twice", () => {
  const messages = historyMessages(events, [execution], "session", "e1");
  assert.equal(messages.length, 1);
  assert.equal(messages[0].role, "user");
  assert.equal(messages[0].attachments?.length, 1);
});

test("failed executions and calls without retained outcomes do not become successful replies", () => {
  const messages = historyMessages(
    events.slice(0, 3),
    [{ ...execution, status: "failed" }],
    "session",
  );
  assert.equal(messages[1].status?.type, "incomplete");
  const content = messages[1].content;
  if (typeof content === "string") throw new Error("Expected structured content");
  const tool = content[1];
  assert.ok(tool.type === "tool-call");
  assert.equal(tool.isError, true);
  assert.match(String(tool.result), /Outcome unknown/);
});

test("a reused call ID in another execution cannot replace an earlier tool result", () => {
  const second = events.map((event) => ({
    ...event,
    id: `second-${event.id}`,
    execution_id: "e2",
    content: event.content.map((item) =>
      item.type === "tool_result" ? { ...item, output: "second result" } : item,
    ),
  }));
  const messages = historyMessages(
    [...events, ...second],
    [execution, { ...execution, execution_id: "e2" }],
    "session",
  );
  assert.equal(messages.length, 4);
  for (const [index, expected] of [
    [1, "fn main() {}"],
    [3, "second result"],
  ] as const) {
    const content = messages[index].content;
    if (typeof content === "string") throw new Error("Expected structured content");
    const call = content[1];
    assert.ok(call.type === "tool-call");
    assert.equal(call.result, expected);
  }
});

test("legacy servers keep readable text history without inventing missing tool metadata", async (t) => {
  t.mock.method(globalThis, "fetch", async (path: string) =>
    path.includes("/history?")
      ? new Response(null, { status: 404 })
      : Response.json({
          messages: [
            {
              id: "old",
              execution_id: "legacy",
              timestamp: 2,
              role: "assistant",
              content: "Old answer",
            },
          ],
          next_cursor: null,
        }),
  );
  const messages = await loadHistory("session");
  assert.equal(messages.length, 1);
  assert.equal(messages[0].metadata?.custom?.historyStatus, "unknown");
});

test("provider reasoning is replayed separately from the assistant answer", () => {
  const page: HistoryEvent[] = [
    {
      id: "thinking",
      execution_id: "e1",
      timestamp_ms: 1100,
      content: [
        { type: "reasoning", text: "Inspect the project." },
        { type: "message", role: "assistant", text: "Done." },
      ],
    },
  ];
  const messages = historyMessages(page, [execution], "session");
  assert.deepEqual(messages[0].content, [
    { type: "reasoning", text: "Inspect the project." },
    { type: "text", text: "Done." },
  ]);
  const plain = historyMessages(
    [{ ...page[0], content: [{ type: "message", role: "assistant", text: "Done." }] }],
    [execution],
    "session",
  );
  assert.equal(
    typeof plain[0].content !== "string" &&
      plain[0].content.some((part) => part.type === "reasoning"),
    false,
  );
});

test("history distinguishes queued calls and retains cancellation for repaired pending results", () => {
  const call = events[2].content[0];
  assert.equal(call.type, "tool_call");
  if (call.type !== "tool_call") throw new Error("Expected tool call");
  const queued = {
    ...events[2],
    content: [
      { ...call, presentation: { ...call.presentation!, started_at: null, finished_at: null } },
    ],
  };
  const running = historyMessages(
    [events[0], queued],
    [{ ...execution, status: "running", finished_at: null }],
    "session",
  );
  const content = running.at(-1)!.content;
  assert.ok(typeof content !== "string" && content[0].type === "tool-call");
  assert.equal(content[0].providerMetadata?.kyler?.state, "queued");
  const repaired: HistoryEvent = {
    ...events[3],
    content: [
      {
        type: "tool_result",
        call_id: "c1",
        name: "read_file",
        status: "error",
        output: "Execution interrupted; tool outcome is unknown.",
        output_bytes: 48,
        truncated: false,
      },
    ],
  };
  const cancelled = historyMessages(
    [events[0], queued, repaired],
    [{ ...execution, status: "cancelled" }],
    "session",
  ).at(-1)!.content;
  assert.ok(typeof cancelled !== "string" && cancelled[0].type === "tool-call");
  assert.equal(cancelled[0].providerMetadata?.kyler?.state, "cancelled");
  assert.equal(cancelled[0].isError, false);
});
