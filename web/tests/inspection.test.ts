import assert from "node:assert/strict";
import test from "node:test";
import {
  contextUsage,
  contextComponents,
  formatCheckpointTokens,
  tokenCount,
  mergeRollout,
  type RequestSnapshot,
  type RolloutItem,
} from "../lib/kyler/inspection.ts";
import { permittedPath } from "../lib/kyler/server.ts";

const request: RequestSnapshot = {
  execution_id: "execution",
  step: 2,
  timestamp_ms: 1000,
  budget: {
    model: "fixed-model",
    context_tokens: 1000,
    sliding_window_tokens: 600,
    input_limit: 800,
    output_tokens: 100,
    safety_tokens: 20,
  },
  estimated_input_tokens: 400,
  provider_input_tokens: null,
  components: [],
  retrieved_memory_keys: [],
  visible_events: 4,
  window_start: 0,
};

test("context occupancy uses the captured request budget and distinguishes estimates", () => {
  assert.equal(contextUsage(null), null);
  assert.deepEqual(contextUsage(request), {
    tokens: 400,
    capacity: 1000,
    percent: 40,
    estimated: true,
  });
  assert.deepEqual(contextUsage({ ...request, provider_input_tokens: 250 }), {
    tokens: 250,
    capacity: 1000,
    percent: 25,
    estimated: false,
  });
  assert.equal(contextUsage({ ...request, provider_input_tokens: 0 })?.tokens, 0);
});

test("rollout polling deduplicates sequence IDs and keeps committed order", () => {
  const item = (seq: number, title = "Event"): RolloutItem => ({
    seq,
    title,
    execution_id: "run",
    timestamp_ms: seq,
    kind: "tool",
    status: null,
  });
  assert.deepEqual(
    mergeRollout([item(2), item(1)], [item(2, "Updated"), item(3)]).map((value) => [
      value.seq,
      value.title,
    ]),
    [
      [1, "Event"],
      [2, "Updated"],
      [3, "Event"],
    ],
  );
});

test("inspection proxy exposes only session-scoped read operations", () => {
  for (const path of [
    "sessions/s1/rollout",
    "sessions/s1/rollout/12",
    "sessions/s1/context",
    "sessions/s1/memory/inspect",
  ]) {
    assert.equal(permittedPath(path, "GET"), true);
    assert.equal(permittedPath(path, "POST"), false);
  }
  assert.equal(permittedPath("sessions/s1/rollout/../../config", "GET"), false);
  assert.equal(permittedPath("rollout", "GET"), false);
});

test("token displays use decimal k without grouping, rounding loss, or changing units", () => {
  for (const [raw, display] of [
    [0, "0k"],
    [1, "0.001k"],
    [999, "0.999k"],
    [1000, "1k"],
    [1024, "1.024k"],
    [1248, "1.248k"],
    [8192, "8.192k"],
    [128000, "128k"],
    [131072, "131.072k"],
    [1000000, "1000k"],
    [Number.MAX_SAFE_INTEGER, "9007199254740.991k"],
  ] as const)
    assert.equal(tokenCount(raw), display);
  for (const invalid of [null, undefined, NaN, Infinity, -1, 1.5]) {
    assert.equal(tokenCount(invalid), "—");
  }
});

test("context composition reconciles estimates with actual input without mutating the snapshot", () => {
  const snapshot: RequestSnapshot = {
    ...request,
    provider_input_tokens: 250,
    components: [
      { name: "system", estimated_tokens: 101 },
      { name: "tools", estimated_tokens: 201 },
      { name: "memory", estimated_tokens: 0 },
      { name: "conversation", estimated_tokens: 98 },
    ],
  };
  assert.deepEqual(
    contextComponents(snapshot).map((p) => p.estimated_tokens),
    [63, 126, 0, 61],
  );
  assert.equal(
    snapshot.components.reduce((sum, p) => sum + p.estimated_tokens, 0),
    400,
  );
  const tiny = contextComponents({ ...snapshot, provider_input_tokens: 1 });
  assert.equal(
    tiny.reduce((sum, p) => sum + p.estimated_tokens, 0),
    1,
  );
  assert.deepEqual(
    contextComponents({ ...snapshot, provider_input_tokens: null }),
    snapshot.components,
  );
  assert.equal(contextUsage({ ...request, provider_input_tokens: -1 })?.estimated, true);
  assert.equal(contextUsage({ ...request, estimated_input_tokens: NaN }), null);
  assert.equal(
    contextUsage({ ...request, budget: { ...request.budget, context_tokens: 0 } }),
    null,
  );
});

test("rollout formatting touches only trusted token metadata and preserves raw data", () => {
  const usage = { prompt_tokens: 1000, completion_tokens: 24, total_tokens: 1024 };
  const record = {
    type: "execution_finished",
    payload: {
      execution: {
        usage,
        subagent_usage: usage,
        current_step: 2,
        state: { total_tokens: 9000 },
        events: [{ arguments: { total_tokens: 8000 } }],
      },
    },
  };
  const displayed = formatCheckpointTokens(record);
  const execution = (displayed.payload as typeof record.payload).execution;
  assert.deepEqual(execution.usage, {
    prompt_tokens: "1k",
    completion_tokens: "0.024k",
    total_tokens: "1.024k",
  });
  assert.equal(execution.current_step, 2);
  assert.equal(execution.state.total_tokens, 9000);
  assert.equal(execution.events[0].arguments.total_tokens, 8000);
  assert.equal(record.payload.execution.usage.total_tokens, 1024);
  const context = formatCheckpointTokens({
    type: "context_updated",
    payload: {
      memory: {
        last_input_tokens: 1024,
        window_start: 3,
        last_request: {
          provider_input_tokens: null,
          budget: { context_tokens: 128000 },
          components: [{ name: "system", estimated_tokens: 999 }],
        },
      },
    },
  });
  assert.match(JSON.stringify(context), /128k/);
  assert.match(JSON.stringify(context), /0\.999k/);
  assert.match(JSON.stringify(context), /—/);
  const event = {
    type: "event_recorded",
    payload: { event: { arguments: { total_tokens: 1000 } } },
  };
  assert.deepEqual(formatCheckpointTokens(event), event);
});
