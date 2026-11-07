import assert from "node:assert/strict";
import test from "node:test";
import { loadWorkspaceFiles } from "../lib/kyler/workspace-files.ts";
import { KylerApiError } from "../lib/kyler/client.ts";

const signal = () => new AbortController().signal;

test("session files filter other owners even when a legacy server aggregates a workspace", async (t) => {
  const files = {
    inputs: [
      { session_id: "owner", attachment: { id: "input" } },
      { session_id: "current", attachment: { id: "current-input" } },
    ],
    outputs: [
      { id: "output", session_id: "owner" },
      { id: "current-output", session_id: "current" },
    ],
  };
  t.mock.method(globalThis, "fetch", async (path: string) => {
    assert.equal(path, "/api/kyler/sessions/current/workspace-files");
    return Response.json(files);
  });
  assert.deepEqual(await loadWorkspaceFiles("current", signal()), {
    inputs: [files.inputs[1]],
    outputs: [files.outputs[1]],
  });
});

test("legacy services fall back to session artifacts and attachments after a workspace route 404", async (t) => {
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("workspace-files")) return new Response(null, { status: 404 });
    if (path.endsWith("artifacts")) return Response.json([{ id: "output", session_id: "s1" }]);
    if (path.endsWith("attachments")) return Response.json([{ id: "input", name: "notes.txt" }]);
    throw new Error("Unexpected request");
  });
  assert.deepEqual(await loadWorkspaceFiles("s1", signal()), {
    inputs: [{ session_id: "s1", attachment: { id: "input", name: "notes.txt" } }],
    outputs: [{ id: "output", session_id: "s1" }],
  });
});

test("an attachment failure does not suppress otherwise downloadable artifacts", async (t) => {
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("artifacts")) return Response.json([{ id: "output", session_id: "s1" }]);
    return new Response(null, { status: 404 });
  });
  const files = await loadWorkspaceFiles("s1", signal());
  assert.equal(files.outputs.length, 1);
  assert.deepEqual(files.inputs, []);
});

test("authentication failures are not disguised as empty workspace contents or bypassed by fallback", async (t) => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return new Response(null, { status: 403 });
  });
  await assert.rejects(
    loadWorkspaceFiles("s1", signal()),
    (error: unknown) => error instanceof KylerApiError && error.status === 403,
  );
  assert.equal(calls, 1);
});
