import assert from "node:assert/strict";
import test from "node:test";
import { parsePatchResult } from "../lib/kyler/patch-changes.ts";

const result = {
  kind: "patch_result",
  status: "success",
  files: [
    { path: "src/game.ts", operation: "modified", added: 12, removed: 4 },
    { path: "after.ts", previous_path: "before.ts", operation: "renamed", added: 0, removed: 0 },
  ],
};

test("structured patch results decode from live events and saved output identically", () => {
  assert.deepEqual(parsePatchResult(result), result);
  assert.deepEqual(parsePatchResult(JSON.stringify(result)), result);
  assert.deepEqual(
    parsePatchResult({ ...result, status: "partial", error: "Inspect failed file" })?.files,
    result.files,
  );
});

test("legacy, truncated or malformed output never produces guessed counts", () => {
  for (const value of [
    "Applied patch to 1 file",
    '{"kind":',
    undefined,
    { ...result, error: {} },
    { ...result, status: "running" },
    { ...result, files: [{ ...result.files[0], added: -1 }] },
    { ...result, files: [{ ...result.files[0], operation: "unknown" }] },
  ]) {
    assert.equal(parsePatchResult(value), undefined);
  }
});

test("saved patch metadata survives a truncated history output preview", async () => {
  const { historyMessages } = await import("../lib/kyler/history.ts");
  const messages = historyMessages(
    [
      {
        id: "event",
        execution_id: "execution",
        timestamp_ms: 1000,
        content: [
          {
            type: "tool_result",
            call_id: "patch",
            name: "apply_patch",
            status: "success",
            patch: JSON.parse(JSON.stringify(result)),
            output: '{"kind":"patch_result",',
            output_bytes: 40000,
            truncated: true,
          },
        ],
      },
    ],
    [],
    "session",
  );
  const content = messages[0]?.content;
  assert.ok(Array.isArray(content));
  const part = content.find((part) => part.type === "tool-call");
  assert.deepEqual(part?.providerMetadata?.kyler?.patch, result);
});

test("composer follows confirmed live patches without carrying history into a new turn", async () => {
  const { latestComposerPatch } = await import("../lib/kyler/patch-changes.ts");
  const completed = {
    type: "tool-call",
    toolName: "apply_patch",
    toolCallId: "first",
    result: JSON.stringify(result),
    providerMetadata: { kyler: { state: "finished", patch: result } },
  };
  const messages = [
    { id: "user", role: "user", content: [] },
    { id: "answer", role: "assistant", content: [completed] },
  ];
  assert.deepEqual(latestComposerPatch(messages)?.result, result);
  const pending = {
    ...completed,
    toolCallId: "next",
    result: undefined,
    providerMetadata: { kyler: { state: "running" } },
  };
  assert.deepEqual(
    latestComposerPatch([...messages, { id: "pending", role: "assistant", content: [pending] }])
      ?.result,
    result,
  );
  const partial = { ...result, status: "partial", error: "Inspect failed file" };
  assert.equal(
    latestComposerPatch([
      ...messages,
      {
        id: "partial",
        role: "assistant",
        content: [
          { ...completed, toolCallId: "next", providerMetadata: { kyler: { patch: partial } } },
        ],
      },
    ])?.result.status,
    "partial",
  );
  assert.equal(
    latestComposerPatch([...messages, { id: "new-user", role: "user", content: [] }]),
    undefined,
  );
  assert.equal(
    latestComposerPatch([
      {
        ...messages[1],
        content: [
          {
            ...completed,
            providerMetadata: { kyler: { historyOutput: JSON.stringify(result), patch: result } },
          },
        ],
      },
    ]),
    undefined,
  );
  const failed = {
    ...completed,
    toolCallId: "failed",
    result: "Permission denied",
    isError: true,
    providerMetadata: { kyler: { state: "finished" } },
  };
  assert.deepEqual(
    latestComposerPatch([...messages, { role: "assistant", content: [failed] }])?.result,
    result,
  );
  const nextTurn = latestComposerPatch([
    { role: "user", content: [] },
    { id: "new-answer", role: "assistant", content: [completed] },
  ]);
  assert.notEqual(nextTurn?.key, latestComposerPatch(messages)?.key);
});
