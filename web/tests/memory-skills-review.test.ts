import {
  draftFilesReviewed,
  skillProposal,
  matchesSkillProposal,
  type DraftDetails,
} from "../lib/kyler/plugins.ts";
import assert from "node:assert/strict";
import test from "node:test";
import { canActivateMemory, type MemoryEntry } from "../lib/kyler/memory.ts";
import { permittedPath } from "../lib/kyler/server.ts";

test("review activation excludes expired and sensitive memory", () => {
  const entry = { metadata: { sensitive: false, expires_at: null } } as MemoryEntry;
  assert.equal(canActivateMemory(entry, 100), true);
  assert.equal(
    canActivateMemory({ ...entry, metadata: { ...entry.metadata, expires_at: 100 } }, 100),
    false,
  );
  assert.equal(
    canActivateMemory({ ...entry, metadata: { ...entry.metadata, sensitive: true } }, 100),
    false,
  );
});
test("publishing requires opening every draft file, including provenance and scripts", () => {
  const files = {
    "SKILL.md": "instructions",
    "references/learning-source.json": "{}",
    "scripts/run.sh": "never execute during review",
  };
  assert.equal(draftFilesReviewed(files, ["SKILL.md"]), false);
  assert.equal(draftFilesReviewed(files, Object.keys(files)), true);
  assert.equal(draftFilesReviewed({}, []), false);
});
test("review proxy exposes only session-bound review operations", () => {
  for (const [path, method] of [
    ["sessions/s1/memory/entries", "GET"],
    ["sessions/s1/memory/entry", "GET"],
    ["sessions/s1/memory/review", "POST"],
    ["sessions/s1/skills/drafts/test-skill", "GET"],
    ["sessions/s1/skills/drafts/test-skill/publish", "POST"],
    ["sessions/s1/skills/drafts/test-skill/discard", "POST"],
  ]) {
    assert.equal(permittedPath(path, method), true);
  }
  assert.equal(permittedPath("memory/entries", "GET"), false);
  assert.equal(permittedPath("sessions/s1/memory/review", "GET"), false);
  assert.equal(permittedPath("sessions/s1/skills/drafts/test-skill/execute", "POST"), false);
});

test("standalone learning routes are retired", () => {
  assert.equal(permittedPath("sessions/s1/learning/memories", "GET"), false);
  assert.equal(permittedPath("sessions/s1/learning/drafts/test-skill/publish", "POST"), false);
});

test("skill review cards only accept completed memory results and survive history replay", () => {
  const draft = {
    name: "rust-validation",
    description: "Validate Rust changes",
    fingerprint: "a".repeat(64),
    resources: [],
  };
  const result = JSON.stringify({ draft });
  const live = { type: "tool-call", toolName: "memory", result };
  assert.deepEqual(skillProposal(live), draft);
  assert.deepEqual(
    skillProposal({
      ...live,
      result: undefined,
      providerMetadata: { kyler: { historyOutput: result } },
    }),
    draft,
  );
  assert.equal(skillProposal({ ...live, isError: true }), undefined);
  assert.equal(skillProposal({ ...live, toolName: "run_command" }), undefined);
  assert.equal(skillProposal({ type: "text", result }), undefined);
  assert.equal(skillProposal({ ...live, result: "incomplete JSON" }), undefined);
  assert.equal(
    skillProposal({ ...live, result: JSON.stringify({ draft: { ...draft, name: "../escape" } }) }),
    undefined,
  );
  const details = {
    scope: "scope",
    draft: {
      info: draft,
      files: {
        "references/learning-source.json": JSON.stringify({
          created_by_session: "s1",
          created_by_tool_call: "c1",
        }),
      },
    },
    installed_fingerprint: null,
    review_status: "pending",
  } as DraftDetails;
  assert.equal(matchesSkillProposal(details, draft, "s1", "c1"), true);
  assert.equal(matchesSkillProposal(details, draft, "s2", "c1"), false);
  assert.equal(matchesSkillProposal(details, draft, "s1", "c2"), false);
  assert.equal(
    matchesSkillProposal(details, { ...draft, fingerprint: "b".repeat(64) }, "s1", "c1"),
    false,
  );
  assert.equal(
    matchesSkillProposal({ ...details, draft: { ...details.draft, files: {} } }, draft, "s1", "c1"),
    false,
  );
});
