import assert from "node:assert/strict";
import test from "node:test";
import { createDraftSession, discardDraft } from "../lib/kyler/drafts.ts";

test("new chat precreates a real draft and lets the server bind its default workspace", async (t) => {
  const bodies: unknown[] = [];
  t.mock.method(globalThis, "fetch", async (url: string, init: RequestInit) => {
    assert.equal(url, "/api/kyler/sessions");
    assert.equal(init.method, "POST");
    bodies.push(JSON.parse(init.body as string));
    return Response.json({
      session_id: `server-${bodies.length}`,
      draft: true,
      workspace_root: "/default",
    });
  });
  assert.equal((await createDraftSession()).session_id, "server-1");
  assert.equal((await createDraftSession(null)).workspace_root, "/default");
  await createDraftSession("/project");
  assert.deepEqual(bodies, [
    { draft: true, workspace_mode: "project" },
    { draft: true, workspace_mode: "project" },
    { draft: true, workspace_mode: "project", workspace_root: "/project" },
  ]);
});

test("abandoned drafts are removed only after confirming they are inactive and unpublished", async (t) => {
  for (const [draft, status, removable] of [
    [true, null, true],
    [true, "failed", true],
    [false, "completed", false],
    [true, "starting", false],
    [true, "running", false],
    [true, "cancelling", false],
  ] as const) {
    const methods: string[] = [];
    const mock = t.mock.method(globalThis, "fetch", async (_url: string, init?: RequestInit) => {
      methods.push(init?.method ?? "GET");
      return init?.method === "DELETE"
        ? new Response(null, { status: 204 })
        : Response.json({ session_id: "draft", draft, status });
    });
    await discardDraft("draft");
    assert.deepEqual(methods, removable ? ["GET", "DELETE"] : ["GET"]);
    mock.mock.restore();
  }
});

test("a cleanup conflict never interrupts navigation", async (t) => {
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({ message: "Busy" }, { status: 409 }),
  );
  await discardDraft("draft");
});
