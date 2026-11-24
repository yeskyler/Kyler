import assert from "node:assert/strict";
import test from "node:test";
import type { Skill } from "../lib/kyler/types.ts";
const review: Skill = {
  id: "review",
  name: "review-agent",
  display_name: "Review Agent",
  source: "builtin",
  status: "enabled",
  path: "builtin://review-agent",
  fingerprint: "fixture",
  resources: [],
  description: "Review code",
  error: null,
};
import {
  commandDraft,
  matchingCommands,
  opensCommandMenu,
  commandRequest,
  reviewSkillDraft,
} from "../lib/kyler/session-commands.ts";

test("slash commands require a typed slash in an empty draft", () => {
  assert.equal(opensCommandMenu("", "/", "insertText"), true);
  assert.equal(opensCommandMenu("", "/", "insertFromPaste"), false);
  assert.equal(opensCommandMenu("", "/Users/kyler", "insertFromPaste"), false);
  assert.equal(opensCommandMenu("See ", "See /", "insertText"), false);
  assert.deepEqual(
    matchingCommands("/", [review]).map(({ name }) => name),
    ["review", "compact", "memory"],
  );
  assert.deepEqual(
    matchingCommands("/RE", [review]).map(({ name }) => name),
    ["review"],
  );
  assert.deepEqual(matchingCommands("/Users"), []);
  assert.deepEqual(matchingCommands("/review notes"), []);
});

test("review selects the same skill reference as the plugin menu without submitting a command", () => {
  assert.equal(reviewSkillDraft("/review"), "@review-agent ");
  assert.equal(reviewSkillDraft("/review "), "@review-agent ");
  assert.equal(
    reviewSkillDraft("@skill-creator @connector:github /review"),
    "@skill-creator @connector:github @review-agent ",
  );
  assert.equal(reviewSkillDraft("@review-agent /review"), "@review-agent ");
  assert.equal(reviewSkillDraft("$review-agent /review"), "$review-agent ");
  assert.equal(reviewSkillDraft("Existing draft"), "Existing draft");
  assert.deepEqual(commandRequest("compact"), { type: "compact" });
  assert.deepEqual(commandRequest("memory"), { type: "memory" });
});

test("plugin references remain in the draft when a command is selected", () => {
  const prefix = "@review-helper @connector:github ";
  assert.equal(opensCommandMenu(prefix, prefix + "/", "insertText"), true);
  assert.deepEqual(commandDraft(prefix + "/review"), { prefix, token: "/review" });
  assert.equal(opensCommandMenu("Draft " + prefix, "Draft " + prefix + "/", "insertText"), false);
});
