import assert from "node:assert/strict";
import test from "node:test";
import {
  pendingMessageQuestion,
  questionResponse,
  mergeQuestions,
  answerLabel,
  type QuestionRecord,
} from "../lib/kyler/questions.ts";
import { permittedPath } from "../lib/kyler/server.ts";
const record: QuestionRecord = {
  id: "q1",
  session_id: "s1",
  execution_id: "e1",
  tool_call_id: "call",
  status: "pending",
  answers: {},
  created_at: 100,
  resolved_at: null,
  questions: [
    {
      id: "format",
      question: "Choose format",
      type: "single_select",
      options: [{ id: "pdf", label: "PDF", recommended: true }],
    },
  ],
};
test("questions require an explicit answer and always permit custom input", () => {
  assert.equal(questionResponse(record, {}), null);
  assert.equal(
    questionResponse(record, { format: { selected: [], text: "", custom: false } }),
    null,
  );
  assert.deepEqual(
    questionResponse(record, {
      format: { selected: ["pdf"], text: "stale custom text", custom: false },
    }),
    { action: "submit", answers: { format: { selected: ["pdf"], text: "" } } },
  );
  assert.deepEqual(
    questionResponse(record, { format: { selected: ["pdf"], text: "PowerPoint", custom: true } }),
    { action: "submit", answers: { format: { selected: [], text: "PowerPoint" } } },
  );
  assert.equal(
    questionResponse(record, { format: { selected: ["foreign"], text: "", custom: false } }),
    null,
  );
});
test("multi-select and text answers are bounded and retain clear history labels", () => {
  const multiple: QuestionRecord = {
    ...record,
    questions: [
      { ...record.questions[0], type: "multi_select" },
      { id: "notes", question: "Notes", type: "text", options: [] },
    ],
  };
  assert.deepEqual(
    questionResponse(multiple, {
      format: { selected: ["pdf"], text: "Also HTML", custom: true },
      notes: { selected: [], text: "Keep it concise", custom: true },
    }),
    {
      action: "submit",
      answers: {
        format: { selected: ["pdf"], text: "Also HTML" },
        notes: { selected: [], text: "Keep it concise" },
      },
    },
  );
  assert.equal(
    questionResponse(record, { format: { selected: [], text: "😀".repeat(1001), custom: true } }),
    null,
  );
  assert.equal(answerLabel(record.questions[0], { selected: ["pdf"], text: "" }), "PDF");
});
test("late pending snapshots cannot resurrect an answered question", () => {
  const answered = {
    ...record,
    status: "answered" as const,
    resolved_at: 200,
    answers: { format: { selected: ["pdf"], text: "" } },
  };
  assert.deepEqual(mergeQuestions([answered], [record]), [answered]);
  assert.deepEqual(mergeQuestions([record], [answered]), [answered]);
});
test("question proxy allows only scoped reads and explicit answers", () => {
  assert.equal(permittedPath("sessions/s1/questions", "GET"), true);
  assert.equal(permittedPath("sessions/s1/questions/q1/response", "POST"), true);
  assert.equal(permittedPath("sessions/s1/questions", "POST"), false);
  assert.equal(permittedPath("sessions/s1/questions/../q1/response", "POST"), false);
});

test("question identifiers cannot access or mutate object prototypes", () => {
  const unusual: QuestionRecord = {
    ...record,
    questions: [{ ...record.questions[0], id: "__proto__" }],
  };
  assert.equal(questionResponse(unusual, {}), null);
  const response = questionResponse(
    unusual,
    Object.fromEntries([["__proto__", { selected: ["pdf"], text: "", custom: false }]]),
  );
  assert.equal(
    JSON.stringify(response),
    '{"action":"submit","answers":{"__proto__":{"selected":["pdf"],"text":""}}}',
  );
});

test("pending questions appear only in their originating assistant message", () => {
  assert.equal(pendingMessageQuestion([record], "e1", []), record);
  assert.equal(pendingMessageQuestion([record], "older", [record.tool_call_id]), undefined);
  assert.equal(pendingMessageQuestion([record], undefined, [record.tool_call_id]), record);
  assert.equal(pendingMessageQuestion([record], undefined, []), undefined);
  assert.equal(pendingMessageQuestion([{ ...record, status: "answered" }], "e1", []), undefined);
});
