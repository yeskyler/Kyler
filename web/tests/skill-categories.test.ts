import assert from "node:assert/strict";
import test from "node:test";
import { matchingCommands, skillCommandDraft } from "../lib/kyler/session-commands.ts";
import { filterSkills, skillCategory } from "../lib/kyler/plugins.ts";
import type { Skill } from "../lib/kyler/types.ts";
const skill = (name: string, source: Skill["source"], status = "enabled"): Skill => ({
  id: `${source}-${name}`,
  name,
  display_name: name === "review-agent" ? "Review Agent" : undefined,
  source,
  status,
  path: source,
  fingerprint: "fixture",
  resources: [],
  description: name,
  error: null,
});

test("system and user categories retain personal and project origins", () => {
  const items = [
    skill("review-agent", "builtin"),
    skill("personal", "user"),
    skill("project", "project"),
  ];
  assert.deepEqual(items.map(skillCategory), ["system", "user", "user"]);
  const user = items.filter((item) => skillCategory(item) === "user");
  assert.deepEqual(
    filterSkills(user, "", "project", "all").map((item) => item.name),
    ["project"],
  );
  assert.deepEqual(
    filterSkills(items, "Review Agent", "all", "all").map((item) => item.name),
    ["review-agent"],
  );
});

test("slash entries are grouped, searchable, and omit unavailable skills", () => {
  const items = [
    skill("project", "project"),
    skill("review-agent", "builtin"),
    skill("personal", "user"),
    skill("off", "builtin", "disabled"),
    skill("old", "builtin", "shadowed"),
  ];
  const entries = matchingCommands("/", items);
  assert.deepEqual(
    entries.map((entry) => entry.group),
    ["System skills", "User skills", "User skills", "Conversation", "Conversation"],
  );
  assert.deepEqual(
    entries.filter((entry) => entry.kind === "skill").map((entry) => entry.name),
    ["review", "personal", "project"],
  );
  assert.equal(matchingCommands("/agent", items)[0]?.name, "review");
  assert.deepEqual(matchingCommands("/off", items), []);
  assert.deepEqual(
    matchingCommands("/project", items).map((entry) => entry.group),
    ["User skills"],
  );
});

test("reserved command names cannot turn skill selection into a session operation", () => {
  const entries = matchingCommands("/", [skill("compact", "user"), skill("review", "project")]);
  const user = entries.filter((entry) => entry.kind === "skill");
  assert.deepEqual(
    user.map((entry) => entry.name),
    ["skill:compact", "skill:review"],
  );
  assert.equal(
    skillCommandDraft("@review-agent /skill:compact", "compact"),
    "@review-agent @compact ",
  );
  assert.equal(skillCommandDraft("@compact /skill:compact", "compact"), "@compact ");
  assert.equal(entries.find((entry) => entry.name === "compact")?.kind, "conversation");
});

test("slash selections accept the same hyphenated identifiers as backend skill packages", () => {
  const item = skill("team--review-2", "user");
  assert.equal(matchingCommands("/team--review", [item])[0]?.kind, "skill");
  assert.equal(skillCommandDraft("/team--review-2", item.name), "@team--review-2 ");
  assert.equal(skillCommandDraft("@team--review-2 /team--review-2", item.name), "@team--review-2 ");
});

test("an exact command or disabled skill cannot select a different fuzzy match", () => {
  const items = [skill("compact", "user"), skill("review-agent", "builtin", "disabled")];
  assert.deepEqual(
    matchingCommands("/compact", items).map((entry) => entry.kind),
    ["conversation"],
  );
  assert.deepEqual(matchingCommands("/review", items), []);
});
