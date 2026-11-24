import assert from "node:assert/strict";
import test from "node:test";
import {
  composerSkills,
  selectComposerSkill,
  skillReferences,
  setComposerSkills,
  completedSkillNames,
} from "../lib/kyler/composer-skills.ts";
import { chatAdapter } from "../lib/kyler/client.ts";
import type { Skill } from "../lib/kyler/types.ts";
const skill = (index = 1): Skill => ({
  id: index.toString(16).padStart(64, "0"),
  fingerprint: "a".repeat(64),
  name: index === 1 ? "review-agent" : `skill-${index}`,
  display_name: index === 1 ? "Review Agent" : undefined,
  status: "enabled",
  source: "builtin",
  path: "builtin",
  resources: [],
  description: "Test skill",
  error: null,
});
function composer(text = "Inspect web/") {
  let state = {
    text,
    runConfig: { custom: { existing: "preserved" } as Record<string, unknown> },
    attachments: ["file-1"],
  };
  return {
    getState: () => state,
    setText: (text: string) => {
      state = { ...state, text };
    },
    setRunConfig: (runConfig: { custom?: Record<string, unknown> }) => {
      state = { ...state, runConfig: { custom: runConfig.custom ?? {} } };
    },
  };
}

test("skill choices are tags with stable references and do not overwrite text or attachments", () => {
  const draft = composer();
  selectComposerSkill(draft, skill());
  selectComposerSkill(draft, skill());
  assert.equal(draft.getState().text, "Inspect web/");
  assert.deepEqual(draft.getState().attachments, ["file-1"]);
  assert.equal(draft.getState().runConfig.custom.existing, "preserved");
  const selected = composerSkills(draft.getState().runConfig.custom.kylerSkills);
  assert.equal(selected.length, 1);
  assert.equal(selected[0].label, "Review Agent");
  assert.deepEqual(skillReferences(selected), [
    { id: skill().id, fingerprint: skill().fingerprint },
  ]);
  selectComposerSkill(draft, skill(), false);
  assert.deepEqual(composerSkills(draft.getState().runConfig.custom.kylerSkills), []);
  assert.equal(draft.getState().text, "Inspect web/");
});

test("manual references convert without leaving duplicate tokens and limits are enforced", () => {
  const draft = composer("@review-agent inspect web/");
  assert.deepEqual(completedSkillNames(draft.getState().text), ["review-agent"]);
  selectComposerSkill(draft, skill());
  assert.equal(draft.getState().text, " inspect web/");
  assert.throws(
    () => selectComposerSkill(draft, { ...skill(2), status: "disabled" }),
    /not enabled/,
  );
  for (let index = 2; index <= 16; index++) selectComposerSkill(draft, skill(index));
  assert.throws(() => selectComposerSkill(draft, skill(17)), /16 skills/);
  assert.equal(composerSkills(draft.getState().runConfig.custom.kylerSkills).length, 16);
  assert.throws(() => skillReferences([{ ...skill(), id: "bad" }]), /Invalid/);
  const sent = composerSkills(draft.getState().runConfig.custom.kylerSkills);
  setComposerSkills(draft, []);
  assert.deepEqual(skillReferences(draft.getState().runConfig.custom.kylerSkills), []);
  setComposerSkills(draft, sent);
  assert.equal(skillReferences(draft.getState().runConfig.custom.kylerSkills).length, 16);
});

test("admission sends clean text, attachment IDs, and exact selected skill versions", async (t) => {
  let admitted = false;
  t.mock.method(globalThis, "fetch", async (path: string, init?: RequestInit) => {
    if (path.endsWith("/runs")) {
      const body = JSON.parse(String(init?.body));
      assert.equal(body.input, "Inspect web/");
      assert.deepEqual(body.attachment_ids, ["file-1"]);
      assert.deepEqual(body.skills, [{ id: skill().id, fingerprint: skill().fingerprint }]);
      assert.equal(JSON.stringify(body).includes("Review Agent"), false);
      admitted = true;
      return Response.json({ run_id: "r1" });
    }
    assert.equal(path, "/api/kyler/runs/r1/events");
    return new Response(
      `data: ${JSON.stringify({ seq: 1, type: "run_completed", output: "Done" })}\n\n`,
      { headers: { "content-type": "text/event-stream" } },
    );
  });
  const draft = composer();
  selectComposerSkill(draft, skill());
  for await (const _ of chatAdapter("s1", () => {}).run({
    messages: [
      {
        role: "user",
        content: [{ type: "text", text: draft.getState().text }],
        attachments: [{ id: "file-1" }],
      },
    ],
    runConfig: draft.getState().runConfig,
    abortSignal: new AbortController().signal,
  } as never) as AsyncIterable<unknown>) {
    /* Consume local fixture. */
  }
  assert.equal(admitted, true);
});
