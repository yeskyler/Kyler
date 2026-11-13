import assert from "node:assert/strict";
import test from "node:test";
import type { ThreadAssistantMessagePart } from "@assistant-ui/react";
import {
  latestDisplayPlan,
  activeDisplayPlan,
  isVisibleMessagePart,
  partitionAssistantParts,
  formatRunDuration,
  groupProcessParts,
  groupToolCategories,
  activityCategory,
} from "../lib/kyler/assistant-display.ts";

const text = (value: string): ThreadAssistantMessagePart => ({ type: "text", text: value });
const tool: ThreadAssistantMessagePart = {
  type: "tool-call",
  toolCallId: "read",
  toolName: "read_file",
  args: {},
  argsText: "{}",
};

test("work stays chronological and only the final answer remains outside the disclosure", () => {
  assert.deepEqual(
    partitionAssistantParts([
      text("Inspecting files"),
      tool,
      text("Checking results"),
      { ...tool, toolCallId: "check" },
      text("Final answer"),
    ]),
    { process: [0, 1, 2, 3], answer: [4] },
  );
});

test("running tools never turn the previous commentary into a final answer", () => {
  assert.deepEqual(partitionAssistantParts([text("Reading"), tool]), {
    process: [0, 1],
    answer: [],
  });
});

test("plain replies stay visible and whitespace does not create an answer", () => {
  assert.deepEqual(partitionAssistantParts([text("Hello")]), { process: [], answer: [0] });
  assert.deepEqual(partitionAssistantParts([tool, text(" \n")]), { process: [0], answer: [] });
  assert.deepEqual(partitionAssistantParts([text("Earlier"), text("Final")]), {
    process: [0],
    answer: [1],
  });
});

test("durations are stable across second and minute boundaries", () => {
  assert.equal(formatRunDuration(-1), "0s");
  assert.equal(formatRunDuration(59000), "59s");
  assert.equal(formatRunDuration(236000), "3m 56s");
});

test("continuous tool trees span model steps without crossing commentary", () => {
  const batchTool = (batch: number): ThreadAssistantMessagePart => ({
    ...tool,
    providerMetadata: { kyler: { batch } },
  });
  const parts = [
    text("Inspecting"),
    batchTool(1),
    batchTool(1),
    batchTool(2),
    text("Checking"),
    batchTool(2),
  ];
  assert.deepEqual(groupProcessParts(parts, [0, 1, 2, 3, 4, 5]), [
    { kind: "content", indices: [0] },
    { kind: "tool", indices: [1, 2, 3] },
    { kind: "content", indices: [4] },
    { kind: "tool", indices: [5] },
  ]);
});

test("missing batch metadata does not duplicate continuous tool category trees", () => {
  assert.deepEqual(
    groupProcessParts([tool, tool], [0, 1]).map((row) => row.indices),
    [[0, 1]],
  );
});

test("parallel category branches merge repeated categories and retain per-category call order", () => {
  const call = (activity: string): ThreadAssistantMessagePart => ({
    ...tool,
    providerMetadata: { kyler: { batch: 1, activity } },
  });
  const parts = [call("explore"), call("explore"), call("edit"), call("explore"), call("unknown")];
  assert.deepEqual(groupToolCategories(parts, [0, 1, 2, 3, 4]), [
    { category: "Explore", indices: [0, 1, 3] },
    { category: "Edit", indices: [2] },
    { category: "Tool", indices: [4] },
  ]);
  assert.equal(activityCategory(undefined), "Tool");
  assert.equal(activityCategory("execute"), "Execute");
});

test("plan popup uses the latest successful structured plan and ignores pending or failed replacements", () => {
  const args = {
    summary: "Build a report",
    steps: [
      { id: "read", content: "Read sources", status: "completed" },
      { id: "write", content: "Write report", status: "in_progress" },
    ],
  };
  const saved = { type: "tool-call", toolName: "update_plan", args, result: "{}" };
  const messages = [
    {
      role: "assistant",
      content: [
        saved,
        { ...saved, args: { ...args, summary: "Failed" }, isError: true },
        { ...saved, args: { ...args, summary: "Pending" }, result: undefined },
      ],
    },
  ];
  assert.deepEqual(latestDisplayPlan(messages), args);
  assert.equal(latestDisplayPlan([{ role: "user", content: [saved] }]), undefined);
  assert.equal(
    latestDisplayPlan([
      { role: "assistant", content: [{ ...saved, args: { summary: "Broken", steps: [{}] } }] },
    ]),
    undefined,
  );
  assert.deepEqual(
    latestDisplayPlan([
      ...messages,
      { role: "assistant", content: [{ ...saved, args: { summary: "Clear", steps: [] } }] },
    ]),
    { summary: "Clear", steps: [] },
  );
});

test("tool generation, queue and approval are hidden until execution starts", () => {
  const thought: ThreadAssistantMessagePart = { type: "reasoning", text: "Thinking" };
  for (const state of ["preparing", "queued", "approval"]) {
    const pending: ThreadAssistantMessagePart = { ...tool, providerMetadata: { kyler: { state } } };
    assert.equal(isVisibleMessagePart(pending), false);
    assert.deepEqual(partitionAssistantParts([thought, pending]), { process: [0], answer: [] });
    assert.deepEqual(partitionAssistantParts([pending]), { process: [], answer: [] });
  }
  const running: ThreadAssistantMessagePart = {
    ...tool,
    providerMetadata: { kyler: { state: "running", startedAt: 1000 } },
  };
  assert.equal(isVisibleMessagePart(running), true);
  assert.deepEqual(partitionAssistantParts([thought, running]), { process: [0, 1], answer: [] });
});

test("completed errors remain visible even when authorization prevented execution", () => {
  const failed: ThreadAssistantMessagePart = {
    ...tool,
    isError: true,
    result: "Permission denied",
    providerMetadata: { kyler: { state: "finished" } },
  };
  assert.equal(isVisibleMessagePart(failed), true);
  assert.deepEqual(partitionAssistantParts([failed, text("Explanation")]), {
    process: [0],
    answer: [1],
  });
  assert.equal(isVisibleMessagePart(tool), true); // Legacy history has no live state metadata.
});

test("cancelling an unstarted call never reveals a preparation card", () => {
  const cancelled: ThreadAssistantMessagePart = {
    ...tool,
    result: "Cancelled.",
    providerMetadata: { kyler: { state: "cancelled" } },
  };
  assert.equal(isVisibleMessagePart(cancelled), false);
  assert.equal(
    isVisibleMessagePart({
      ...cancelled,
      providerMetadata: { kyler: { state: "cancelled", startedAt: 1000 } },
    }),
    true,
  );
});

test("interrupted parameter generation never becomes an executed tool card", () => {
  const interrupted: ThreadAssistantMessagePart = {
    ...tool,
    result: "Outcome unknown; no tool result was received.",
    isError: true,
    providerMetadata: { kyler: { state: "interrupted" } },
  };
  assert.equal(isVisibleMessagePart(interrupted), false);
  assert.deepEqual(partitionAssistantParts([interrupted]), { process: [], answer: [] });
  assert.equal(
    isVisibleMessagePart({
      ...interrupted,
      providerMetadata: { kyler: { state: "interrupted", startedAt: 1000 } },
    }),
    true,
  );
});

test("composer plan follows only the active turn and disappears on completion or termination", () => {
  const steps = [
    { id: "build", content: "Implement feature", status: "in_progress" },
    { id: "test", content: "Verify result", status: "pending" },
  ];
  const planMessage = (items: typeof steps) => ({
    role: "assistant",
    content: [
      {
        type: "tool-call",
        toolName: "update_plan",
        args: { summary: "Deliver feature", steps: items },
        result: "{}",
      },
    ],
  });
  const messages = [{ role: "user", content: [] }, planMessage(steps)];
  assert.deepEqual(activeDisplayPlan(messages, true)?.steps, steps);
  assert.equal(activeDisplayPlan(messages, false), undefined);
  assert.equal(activeDisplayPlan([...messages, { role: "user", content: [] }], true), undefined);
  assert.equal(
    activeDisplayPlan(
      [...messages, planMessage(steps.map((step) => ({ ...step, status: "completed" })))],
      true,
    ),
    undefined,
  );
  assert.equal(activeDisplayPlan([...messages, planMessage([])], true), undefined);
  const updated = steps.map((step, index) => ({
    ...step,
    status: index === 0 ? "completed" : "in_progress",
  }));
  assert.deepEqual(activeDisplayPlan([...messages, planMessage(updated)], true)?.steps, updated);
});
