import { draftFilesReviewed } from "../lib/kyler/plugins.ts";
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
