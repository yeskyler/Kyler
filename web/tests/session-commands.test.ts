import assert from "node:assert/strict";
import test from "node:test";
import {
  commandDraft,
  matchingCommands,
  opensCommandMenu,
  reviewTarget,
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

test("review targets preserve the selected scope and reject revision expressions", () => {
  assert.deepEqual(reviewTarget("uncommitted", ""), { type: "uncommitted" });
  assert.deepEqual(reviewTarget("base_branch", " feature/ui "), {
    type: "base_branch",
    branch: "feature/ui",
  });
  assert.deepEqual(reviewTarget("commit", "c46bb7e"), { type: "commit", commit: "c46bb7e" });
  for (const ref of ["", "-C", "HEAD~1", "a..b", "name with spaces"]) {
    assert.equal(reviewTarget("commit", ref), null);
  }
});

test("plugin references remain in the draft when a command is selected", () => {
  const prefix = "@review-helper @connector:github ";
  assert.equal(opensCommandMenu(prefix, prefix + "/", "insertText"), true);
  assert.deepEqual(commandDraft(prefix + "/review"), { prefix, token: "/review" });
  assert.equal(opensCommandMenu("Draft " + prefix, "Draft " + prefix + "/", "insertText"), false);
});
