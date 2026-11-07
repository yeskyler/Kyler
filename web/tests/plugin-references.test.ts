import assert from "node:assert/strict";
import test from "node:test";
import { appendPluginReference, pluginReference } from "../lib/kyler/plugin-references.ts";

test("plugin selection preserves the draft and inserts explicit references", () => {
  assert.equal(
    appendPluginReference("Create a skill", { kind: "skill", name: "skill-creator" }),
    "Create a skill @skill-creator ",
  );
  assert.equal(
    appendPluginReference("First line\n", { kind: "connector", name: "my_server" }),
    "First line\n@connector:my_server ",
  );
});

test("skill selection deduplicates current and legacy references", () => {
  for (const text of ["@skill-creator draft", "$skill-creator draft"]) {
    assert.equal(appendPluginReference(text, { kind: "skill", name: "skill-creator" }), text);
  }
});

test("references reject invalid plugin identifiers", () => {
  assert.equal(pluginReference({ kind: "connector", name: "api_1" }), "@connector:api_1");
  assert.throws(() => pluginReference({ kind: "skill", name: "../skill" }));
  assert.throws(() => pluginReference({ kind: "connector", name: "server other" }));
});
