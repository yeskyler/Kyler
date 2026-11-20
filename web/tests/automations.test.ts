import assert from "node:assert/strict";
import test from "node:test";
import {
  mergeRuns,
  thinkingForModel,
  scheduleLabel,
  taskTime,
  unfinished,
  workspaceLabel,
  type AutomationRun,
} from "../lib/kyler/automations.ts";

test("automation history updates existing rows without losing older pages", () => {
  const earlier = { id: "a", created_at: 1, status: "running" } as AutomationRun;
  const later = { id: "b", created_at: 2, status: "queued" } as AutomationRun;
  const rows = mergeRuns([earlier, later], [{ ...earlier, status: "completed" }]);
  assert.deepEqual(
    rows.map((row) => [row.id, row.status]),
    [
      ["b", "queued"],
      ["a", "completed"],
    ],
  );
});
test("automation schedules and lifecycle labels preserve custom choices", () => {
  assert.equal(scheduleLabel("0 9 * * 1-5"), "Weekdays at 09:00");
  assert.equal(scheduleLabel("*/15 * * * *"), "*/15 * * * *");
  assert.equal(unfinished("waiting_for_approval"), true);
  assert.equal(unfinished("completed"), false);
  assert.equal(unfinished(undefined), false);
  assert.equal(taskTime(null, "UTC"), "—");
  assert.equal(taskTime(1, "invalid"), "Invalid time zone");
  assert.equal(workspaceLabel("/home/user/project/"), "project");
});

test("automation proxy exposes only the supported scheduler operations", async () => {
  const { permittedPath } = await import("../lib/kyler/server.ts");
  const id = "57ba784b-a984-4c6c-a37c-f0216823e3b7";
  for (const [method, path] of [
    ["GET", "automations"],
    ["POST", "automations"],
    ["POST", "automations/preview"],
    ["PUT", `automations/${id}`],
    ["GET", `automations/${id}/runs`],
    ["POST", `automations/${id}/runs`],
    ["POST", `automation-runs/${id}/cancel`],
  ]) {
    assert.ok(permittedPath(path, method));
  }
  assert.equal(permittedPath(`automations/${id}/shell`, "POST"), false);
  assert.equal(permittedPath(`automation-runs/${id}`, "DELETE"), false);
});

test("automation thinking follows model capabilities and drops unsupported preferences", () => {
  const catalog = {
    default_profile: "openai",
    models: [
      {
        profile: "openai",
        provider: "one",
        model: "reasoning",
        capabilities: {
          reasoning: { levels: ["off", "low", "high", "max"], default: "high", source: "fixture" },
        },
      },
      {
        profile: "siliconflow",
        provider: "two",
        model: "reasoning",
        capabilities: {
          reasoning: { levels: ["off", "high", "max"], default: "high", source: "fixture" },
        },
      },
      { profile: "unknown", provider: "three", model: "unknown" },
    ],
  } satisfies import("../lib/kyler/types.ts").ModelCatalog;
  assert.equal(thinkingForModel(catalog, "openai", "low").level, "low");
  assert.equal(thinkingForModel(catalog, "siliconflow", "low").level, "high");
  assert.equal(thinkingForModel(catalog, "siliconflow", "max").level, "max");
  assert.deepEqual(thinkingForModel(catalog, "unknown", "high"), { levels: [], level: "off" });
});
