import assert from "node:assert/strict";
import test from "node:test";
import {
  thinkingLevels,
  thinkingOptions,
  validateModelSettings,
} from "../lib/kyler/model-settings.ts";
import type { SessionModel, ThinkingState } from "../lib/kyler/types.ts";

function selection(
  status: ThinkingState["status"],
  levels: ThinkingState["levels"],
  level: ThinkingState["level"],
): SessionModel {
  return {
    profile: "one",
    selected: { profile: "one", provider: "fixture", model: "model", revision: 4 },
    active: null,
    error: null,
    thinking: {
      profile: "one",
      revision: 4,
      status,
      supported: status === "supported",
      enabled: level !== "off",
      follows_default: true,
      source: "fixture",
      levels,
      level,
    },
  };
}
test("all seven levels stay visible while availability follows the selected model", () => {
  assert.deepEqual(thinkingLevels, ["off", "low", "medium", "high", "xhigh", "max", "ultra"]);
  const known = validateModelSettings(selection("supported", ["high", "max"], "high"));
  const options = thinkingOptions(known.thinking!);
  assert.equal(options.length, 7);
  assert.deepEqual(
    options.filter((item) => item.available).map((item) => item.level),
    ["high", "max"],
  );
  assert.equal(options.find((item) => item.level === "ultra")?.available, false);
  assert.equal(options.find((item) => item.level === "off")?.available, false);
});
test("unknown, unsupported and failed loads never masquerade as one another", () => {
  const unknown = validateModelSettings(selection("unknown", [], "off"));
  assert.ok(thinkingOptions(unknown.thinking!).every((item) => !item.available));
  const unsupported = validateModelSettings(selection("unsupported", ["off"], "off"));
  assert.deepEqual(
    thinkingOptions(unsupported.thinking!)
      .filter((item) => item.available)
      .map((item) => item.level),
    ["off"],
  );
  assert.ok(thinkingOptions(undefined).every((item) => !item.available));
  assert.throws(() => validateModelSettings({ ...unknown, thinking: null }), /unavailable/);
});
test("mixed model identities and catalog generations are rejected", () => {
  const value = selection("supported", ["high", "ultra"], "ultra");
  assert.equal(validateModelSettings(value).thinking!.level, "ultra");
  assert.throws(
    () => validateModelSettings({ ...value, thinking: { ...value.thinking!, profile: "old" } }),
    /do not match/,
  );
  assert.throws(
    () => validateModelSettings({ ...value, thinking: { ...value.thinking!, revision: 3 } }),
    /do not match/,
  );
  assert.throws(
    () => validateModelSettings({ ...value, thinking: { ...value.thinking!, level: "max" } }),
    /not supported/,
  );
});
