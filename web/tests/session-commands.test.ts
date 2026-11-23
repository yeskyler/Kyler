import assert from "node:assert/strict";
import test from "node:test";
import {
  commandDraft,
  matchingCommands,
  opensCommandMenu,
  commandRequest,
} from "../lib/kyler/session-commands.ts";

test("slash commands require a typed slash in an empty draft", () => {
  assert.equal(opensCommandMenu("", "/", "insertText"), true);
  assert.equal(opensCommandMenu("", "/", "insertFromPaste"), false);
  assert.equal(opensCommandMenu("", "/Users/kyler", "insertFromPaste"), false);
  assert.equal(opensCommandMenu("See ", "See /", "insertText"), false);
  assert.deepEqual(
    matchingCommands("/").map(({ name }) => name),
    ["compact", "review", "memory"],
  );
  assert.deepEqual(
    matchingCommands("/RE").map(({ name }) => name),
    ["review"],
  );
  assert.deepEqual(matchingCommands("/Users"), []);
  assert.deepEqual(matchingCommands("/review notes"), []);
});

test("review defaults to uncommitted changes without a workspace override", () => {
  assert.deepEqual(commandRequest("review"), { type: "review", target: { type: "uncommitted" } });
});

test("plugin references remain in the draft when a command is selected", () => {
  const prefix = "@review-helper @connector:github ";
  assert.equal(opensCommandMenu(prefix, prefix + "/", "insertText"), true);
  assert.deepEqual(commandDraft(prefix + "/review"), { prefix, token: "/review" });
  assert.equal(opensCommandMenu("Draft " + prefix, "Draft " + prefix + "/", "insertText"), false);
});
