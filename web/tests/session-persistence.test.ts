import assert from "node:assert/strict";
import test from "node:test";
import { initialSession, patchPreferences, readPreferences } from "../lib/kyler/preferences.ts";
import { activeSessionRun } from "../lib/kyler/session-recovery.ts";
import { permittedPath } from "../lib/kyler/server.ts";

test("explicit navigation wins over the server's recent session", () => {
  const preferences = { last_session_id: "recent", sidebar_collapsed: true, revision: 2 };
  assert.equal(initialSession("explicit", preferences), "explicit");
  assert.equal(initialSession(null, preferences), "recent");
});
test("a fresh browser loads preferences and discovers the existing run without local storage", async () => {
  const original = globalThis.fetch;
  const paths: string[] = [];
  globalThis.fetch = async (input) => {
    paths.push(String(input));
    return Response.json(
      String(input).endsWith("preferences")
        ? { last_session_id: "session", sidebar_collapsed: true, revision: 1 }
        : {
            run: { run_id: "existing-run", execution_id: "execution", status: "running" },
            input: "Continue work",
          },
    );
  };
  try {
    assert.equal((await readPreferences()).last_session_id, "session");
    assert.equal((await activeSessionRun("session"))?.run.run_id, "existing-run");
    assert.deepEqual(paths, ["/api/kyler/preferences", "/api/kyler/sessions/session/active-run"]);
  } finally {
    globalThis.fetch = original;
  }
});
test("preference patches are ordered and do not send unrelated fields", async () => {
  const original = globalThis.fetch;
  const patches: unknown[] = [];
  globalThis.fetch = async (_, init) => {
    patches.push(JSON.parse(String(init?.body)));
    return Response.json({ revision: patches.length });
  };
  try {
    await Promise.all([
      patchPreferences({ sidebar_collapsed: true }),
      patchPreferences({ last_session_id: "session" }),
      patchPreferences({ sidebar_collapsed: false }),
    ]);
    assert.deepEqual(patches, [
      { sidebar_collapsed: true },
      { last_session_id: "session" },
      { sidebar_collapsed: false },
    ]);
  } finally {
    globalThis.fetch = original;
  }
});
test("shared state routes remain constrained to supported methods", () => {
  assert.equal(permittedPath("preferences", "GET"), true);
  assert.equal(permittedPath("preferences", "PUT"), true);
  assert.equal(permittedPath("preferences", "DELETE"), false);
  assert.equal(permittedPath("sessions/session/active-run", "GET"), true);
  assert.equal(permittedPath("sessions/session/active-run", "POST"), false);
});

test("expired live replay reconnects without cancelling the backend task", async (t) => {
  const { chatAdapter } = await import("../lib/kyler/client.ts");
  let attempts = 0;
  let cancellations = 0;
  t.mock.method(globalThis, "fetch", async (input: string, init?: RequestInit) => {
    if (input.endsWith("/cancel")) {
      cancellations++;
      return Response.json({});
    }
    if (input.endsWith("/events")) {
      if (++attempts === 1) return Response.json({ message: "Expired" }, { status: 410 });
      assert.equal(new Headers(init?.headers).get("last-event-id"), "42");
      return new Response(
        `data: ${JSON.stringify({ type: "run_completed", seq: 43, output: "Complete answer" })}\n\n`,
        { headers: { "content-type": "text/event-stream" } },
      );
    }
    assert.ok(input.endsWith("/runs/existing"));
    return Response.json({
      status: "running",
      last_event_seq: 42,
      execution_id: "execution",
      output: null,
    });
  });
  let last: any;
  for await (const update of chatAdapter("session", () => {}, "existing").run({
    messages: [{ role: "user", content: [{ type: "text", text: "Original" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator) {
    last = update;
  }
  assert.equal(cancellations, 0);
  assert.deepEqual(last.content, [{ type: "text", text: "Complete answer" }]);
});
