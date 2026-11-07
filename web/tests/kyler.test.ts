import type { ChatModelRunResult, PendingAttachment } from "@assistant-ui/react";
import {
  groupWorkspaceSessions,
  sessionActivity,
  workspaceName,
  sessionTitle,
  mergeSessionHistory,
} from "../lib/kyler/sessions.ts";
import assert from "node:assert/strict";
import test from "node:test";
import { chooseLocalDirectory } from "../lib/kyler/local-directory.ts";
import { readEvents } from "../lib/kyler/events.ts";
import { latestDisplayPlan } from "../lib/kyler/assistant-display.ts";
import { api, attachments, chatAdapter } from "../lib/kyler/client.ts";
import { permittedPath, serverSettings, proxyRequest } from "../lib/kyler/server.ts";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

function stream(events: object[], chunkSize = 7) {
  const bytes = new TextEncoder().encode(
    events.map((event) => `data: ${JSON.stringify(event)}\r\n\r\n`).join(""),
  );
  let position = 0;
  return new Response(
    new ReadableStream({
      pull(controller) {
        if (position === bytes.length) {
          controller.close();
          return;
        }
        controller.enqueue(bytes.slice(position, (position += chunkSize)));
      },
    }),
  );
}

test("SSE preserves multibyte text and CRLF frames split at every byte", async () => {
  const events = [
    { type: "text_delta", seq: 1, text: "Hello 世界 🌎" },
    { type: "run_completed", seq: 2, output: "Done" },
  ];
  const actual = [];
  for await (const event of readEvents(stream(events, 1))) actual.push(event);
  assert.deepEqual(actual, events);
});

test("SSE reports expired replay instead of silently ending", async () => {
  await assert.rejects(async () => {
    for await (const _ of readEvents(stream([{ code: "expired", message: "Replay expired" }]))) {
    }
  }, /Replay expired/);
});

test("proxy exposes conversation operations but never service control or traversal", () => {
  assert.equal(permittedPath("sessions/s1/runs", "POST"), true);
  assert.equal(permittedPath("sessions/s1/workspace-files", "GET"), true);
  assert.equal(permittedPath("sessions/s1/workspace-files", "POST"), false);
  assert.equal(permittedPath("runs/r1/events", "GET"), true);
  assert.equal(permittedPath("sessions/s1/attachments", "POST"), true);
  assert.equal(permittedPath("sessions/s1/attachments", "GET"), true);
  assert.equal(permittedPath("sessions/s1/attachments/a1/content", "GET"), true);
  assert.equal(permittedPath("sessions/s1/attachments/a1/content", "POST"), false);
  for (const path of [
    "service/shutdown",
    "../info",
    "runs/../../service/shutdown",
    "https://other.example",
    "sessions/s1/runs/extra",
  ])
    assert.equal(permittedPath(path, "POST"), false);
  assert.equal(permittedPath("runs/r1/cancel", "GET"), false);
});

test("proxy reuses installed TOML endpoint and token with explicit workspace", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "kyler-ui-config-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const file = join(root, "config.toml");
  await writeFile(
    file,
    '[client]\nserver_url = "http://127.0.0.1:30001"\naccess_token = "fake-service-token"\n',
  );
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  delete process.env.KYLER_CLIENT_SERVER_URL;
  delete process.env.KYLER_SERVER_URL;
  delete process.env.KYLER_CLIENT_TOKEN;
  delete process.env.KYLER_SERVER_TOKEN;
  process.env.KYLER_CONFIG = file;
  process.env.KYLER_WORKSPACE = root;
  const settings = await serverSettings();
  assert.equal(settings.url.origin, "http://127.0.0.1:30001");
  assert.equal(settings.token, "fake-service-token");
  assert.equal("workspace" in settings, false);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30003";
  assert.equal((await serverSettings()).url.port, "30003");
});

test("custom upstreams do not inherit the local backend token", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "kyler-upstream-config-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  for (const name of [
    "KYLER_CLIENT_SERVER_URL",
    "KYLER_SERVER_URL",
    "KYLER_CLIENT_TOKEN",
    "KYLER_SERVER_TOKEN",
  ])
    delete process.env[name];
  const file = join(root, "config.toml");
  process.env.KYLER_CONFIG = file;
  await writeFile(
    file,
    '[server]\nbind="0.0.0.0:31001"\naccess_token="local-private-token"\n[client]\nserver_url="http://127.0.0.1:31001"\n',
  );
  assert.equal((await serverSettings()).token, "local-private-token");
  process.env.KYLER_CLIENT_SERVER_URL = "https://example.com";
  assert.equal((await serverSettings()).token, "");
  process.env.KYLER_CLIENT_TOKEN = "explicit-upstream-token";
  assert.equal((await serverSettings()).token, "explicit-upstream-token");
});

test("adapter submits only the new turn and renders ordered tool and text events", async (t) => {
  const calls: { path: string; init?: RequestInit }[] = [];
  const approvals: unknown[] = [];
  t.mock.method(globalThis, "fetch", async (path: string, init?: RequestInit) => {
    calls.push({ path, init });
    if (path.endsWith("/runs")) return Response.json({ run_id: "r1" });
    if (path.endsWith("/events"))
      return stream([
        { seq: 1, type: "text_delta", step: 1, text: "Reading files." },
        { seq: 2, type: "tool_started", call_id: "c1", name: "list_dir", activity: "explore" },
        { seq: 3, type: "approval_updated", approval: { id: "a1", status: "pending" } },
        { seq: 4, type: "tool_finished", call_id: "c1", status: "success" },
        { seq: 5, type: "text_delta", step: 2, text: "Done." },
        { seq: 6, type: "run_completed", output: "Done." },
      ]);
    if (path.includes("tool-calls"))
      return Response.json({ output: "src/", arguments: { path: "/workspace" } });
    throw new Error(`Unexpected request: ${path}`);
  });
  const adapter = chatAdapter("s1", (approval) => approvals.push(approval));
  const result = adapter.run({
    messages: [
      { role: "user", content: [{ type: "text", text: "Old prompt" }], attachments: [] },
      { role: "assistant", content: [{ type: "text", text: "Old answer" }] },
      {
        role: "user",
        content: [{ type: "text", text: "New prompt" }],
        attachments: [{ id: "file1" }],
      },
    ],
    abortSignal: new AbortController().signal,
  } as never);
  const updates = [];
  for await (const update of result as AsyncGenerator<ChatModelRunResult>) updates.push(update);
  const body = JSON.parse(calls[0].init!.body as string);
  assert.equal(body.input, "New prompt");
  assert.deepEqual(body.attachment_ids, ["file1"]);
  assert.equal(
    calls.some((call) => call.path.endsWith("/cancel")),
    false,
  );
  assert.equal(approvals.length, 1);
  const final = updates.at(-1);
  assert.ok(final?.content);
  assert.equal(final.status?.type, "complete");
  assert.deepEqual(
    final.content.map((part: { type: string }) => part.type),
    ["text", "tool-call", "text"],
  );
  assert.equal(final.content[1].type, "tool-call");
  assert.ok(final.content[1].type === "tool-call");
  assert.equal(final.content[1].result, "success");
  assert.equal(
    calls.some((call) => call.path.includes("tool-calls")),
    false,
  );
  assert.equal(final.content[1].isError, false);
  assert.ok(final.content[2].type === "text");
  assert.equal(final.content[2].text, "Done.");
});

test("Stop cancels an admitted backend run even if stopped during admission", async (t) => {
  const abort = new AbortController();
  let cancelled = false;
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/cancel")) {
      cancelled = true;
      return Response.json({});
    }
    if (path.endsWith("/runs")) {
      abort.abort();
      return Response.json({ run_id: "r1" });
    }
    throw new Error("Unexpected stream request");
  });
  await assert.rejects(async () => {
    for await (const _ of chatAdapter("s1", () => {}).run({
      messages: [{ role: "user", content: [{ type: "text", text: "Hello" }], attachments: [] }],
      abortSignal: abort.signal,
    } as never) as AsyncGenerator) {
    }
  });
  assert.equal(cancelled, true);
});

test("file upload sends bytes to the session and returns an attachment reference", async (t) => {
  const file = new File(["example"], "note.txt", { type: "text/plain" });
  t.mock.method(globalThis, "fetch", async (path: string, init?: RequestInit) => {
    assert.equal(path, "/api/kyler/sessions/s1/attachments?filename=note.txt");
    assert.equal(init?.body, file);
    return Response.json({ id: "attachment1" });
  });
  const adapter = attachments("s1");
  const pending = (await adapter.add({ file })) as PendingAttachment;
  assert.equal(pending.id, "attachment1");
  assert.equal((await adapter.send(pending)).status.type, "complete");
});

test("proxy rejects cross-site writes and forwards an explicit workspace with the server credential", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  process.env.KYLER_CLIENT_TOKEN = "private-test-token";
  process.env.KYLER_WORKSPACE = "/test/workspace";
  let fetched = false;
  t.mock.method(globalThis, "fetch", async (url: URL, init: RequestInit) => {
    fetched = true;
    assert.equal(url.href, "http://127.0.0.1:30001/v1/sessions");
    assert.equal(new Headers(init.headers).get("authorization"), "Bearer private-test-token");
    assert.deepEqual(JSON.parse(new TextDecoder().decode(init.body as Uint8Array)), {
      workspace_root: "/chosen/project",
    });
    return Response.json({ session_id: "s1" }, { status: 201 });
  });
  const rejected = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/sessions", {
      method: "POST",
      headers: { origin: "https://evil.example" },
      body: "{}",
    }),
    ["sessions"],
  );
  assert.equal(rejected.status, 403);
  assert.equal(fetched, false);
  const accepted = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/sessions", {
      method: "POST",
      headers: { origin: "http://127.0.0.1:3002", host: "127.0.0.1:3002" },
      body: JSON.stringify({ workspace_root: "/chosen/project" }),
    }),
    ["sessions"],
  );
  assert.equal(accepted.status, 201);
  assert.deepEqual(await accepted.json(), { session_id: "s1" });
  assert.equal(accepted.headers.has("authorization"), false);
});

test("proxy forwards replay cursor and passes SSE bytes without buffering", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  const body = new ReadableStream({
    start(controller) {
      controller.enqueue(
        new TextEncoder().encode('data: {"type":"text_delta","seq":2,"text":"Streaming"}\n\n'),
      );
      controller.close();
    },
  });
  t.mock.method(globalThis, "fetch", async (_: URL, init: RequestInit) => {
    assert.equal(new Headers(init.headers).get("last-event-id"), "1");
    return new Response(body, { headers: { "content-type": "text/event-stream" } });
  });
  const response = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/runs/r1/events", {
      headers: { "last-event-id": "1" },
    }),
    ["runs", "r1", "events"],
  );
  assert.equal(response.body, body);
  assert.equal(response.headers.get("content-type"), "text/event-stream");
  assert.match(await response.text(), /Streaming/);
});

test("reconnecting replays only new sequence IDs without creating another run", async (t) => {
  let streams = 0;
  let admissions = 0;
  t.mock.method(globalThis, "fetch", async (path: string, init?: RequestInit) => {
    if (path.endsWith("/runs")) {
      admissions++;
      return Response.json({ run_id: "r1" });
    }
    if (path.endsWith("/events")) {
      streams++;
      if (streams === 1) return stream([{ type: "text_delta", seq: 1, step: 1, text: "Hello" }]);
      assert.equal(new Headers(init?.headers).get("last-event-id"), "1");
      return stream([
        { type: "text_delta", seq: 1, step: 1, text: "Hello" },
        { type: "text_delta", seq: 2, step: 1, text: " world" },
        { type: "run_completed", seq: 3, output: "Hello world" },
      ]);
    }
    throw new Error("Unexpected request");
  });
  const updates: ChatModelRunResult[] = [];
  for await (const update of chatAdapter("s1", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Hi" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    updates.push(update);
  assert.equal(admissions, 1);
  assert.equal(streams, 2);
  assert.deepEqual(updates.at(-1)?.content, [{ type: "text", text: "Hello world" }]);
});

test("recovering an existing run subscribes without submitting the user turn again", async (t) => {
  t.mock.method(globalThis, "fetch", async (path: string) => {
    assert.equal(path, "/api/kyler/runs/existing/events");
    return stream([
      { type: "text_delta", seq: 1, text: "Recovered" },
      { type: "run_completed", seq: 2, output: "Recovered" },
    ]);
  });
  const updates: ChatModelRunResult[] = [];
  for await (const update of chatAdapter("s1", () => {}, "existing").run({
    messages: [],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    updates.push(update);
  assert.equal(updates.at(-1)?.status?.type, "complete");
});

test("management proxy permits only supported resource actions", () => {
  for (const [path, method] of [
    ["sessions", "GET"],
    ["sessions/s1", "DELETE"],
    ["models", "GET"],
    ["workspaces", "GET"],
    ["workspaces", "POST"],
    ["workspaces/project-id", "DELETE"],
    ["plugins/skills", "GET"],
    ["plugins/skills", "POST"],
    ["plugins/connectors", "GET"],
    ["plugins/connectors", "POST"],
    ["models/settings", "GET"],
    ["models/settings", "PUT"],
    ["models/reload", "POST"],
    ["sessions/s1/model", "PUT"],
    ["sessions/s1/model/settings", "GET"],
    ["sessions/s1/model/settings", "PUT"],
    ["sessions/s1/skills", "POST"],
    ["sessions/s1/skills/abc/enable", "POST"],
    ["sessions/s1/mcp", "POST"],
    ["sessions/s1/mcp/user_server/connect", "POST"],
  ])
    assert.equal(permittedPath(path, method), true);
  for (const path of [
    "sessions/s1/mcp/server/execute",
    "sessions/s1/skills/abc/delete",
    "models/credentials",
    "sessions/s1/workspace",
  ])
    assert.equal(permittedPath(path, "POST"), false);
});

test("workspace selection rejects relative paths and unknown fields before forwarding", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  let requests = 0;
  t.mock.method(globalThis, "fetch", async () => {
    requests++;
    return Response.json({});
  });
  for (const body of [
    { workspace_root: "../outside" },
    { workspace_root: 12 },
    { workspace_root: "/project", privileged: true },
  ]) {
    const response = await proxyRequest(
      new Request("http://localhost:3002/api/kyler/sessions", {
        method: "POST",
        body: JSON.stringify(body),
      }),
      ["sessions"],
    );
    assert.equal(response.status, 400);
  }
  assert.equal(requests, 0);
});

test("session activity distinguishes all execution states and local admission", () => {
  for (const status of ["starting", "running", "cancelling"])
    assert.equal(sessionActivity(status).busy, true);
  assert.equal(sessionActivity("starting").label, "Starting");
  assert.equal(sessionActivity("running").label, "Working");
  assert.equal(sessionActivity("cancelling").label, "Stopping");
  for (const status of ["completed", "failed", "cancelled"]) {
    assert.equal(sessionActivity(status).kind, status);
    assert.equal(sessionActivity(status).busy, false);
  }
  assert.equal(sessionActivity(null).label, "Idle");
  assert.equal(sessionActivity("completed", true).busy, true);
  assert.equal(sessionActivity("cancelling", true).label, "Stopping");
});

test("session deletion forwards to Kyler and surfaces a busy-session rejection", async (t) => {
  t.mock.method(globalThis, "fetch", async (path: string, init?: RequestInit) => {
    assert.equal(path, "/api/kyler/sessions/s1");
    assert.equal(init?.method, "DELETE");
    return Response.json(
      { code: "session_busy", message: "The session already has a running execution." },
      { status: 409 },
    );
  });
  const { api } = await import("../lib/kyler/client.ts");
  await assert.rejects(api("sessions/s1", { method: "DELETE" }), /running execution/);
});

test("model setting edits keep service authorization separate from the model key", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  process.env.KYLER_CLIENT_TOKEN = "PRIVATE_SERVICE_TOKEN";
  t.mock.method(globalThis, "fetch", async (url: URL, init: RequestInit) => {
    assert.equal(url.href, "http://127.0.0.1:30001/v1/sessions/s1/model/settings");
    assert.equal(init.method, "PUT");
    assert.equal(new Headers(init.headers).get("authorization"), "Bearer PRIVATE_SERVICE_TOKEN");
    assert.deepEqual(JSON.parse(new TextDecoder().decode(init.body as Uint8Array)), {
      url: "https://example.com/v1",
      model: "custom-model",
      key: "PRIVATE_MODEL_KEY",
    });
    return Response.json({
      url: "https://example.com/v1",
      model: "custom-model",
      key_configured: true,
    });
  });
  const response = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/sessions/s1/model/settings", {
      method: "PUT",
      headers: { "content-type": "application/json", origin: "http://localhost:3002" },
      body: JSON.stringify({
        url: "https://example.com/v1",
        model: "custom-model",
        key: "PRIVATE_MODEL_KEY",
      }),
    }),
    ["sessions", "s1", "model", "settings"],
  );
  assert.equal(response.status, 200);
  const body = await response.text();
  assert.ok(!body.includes("PRIVATE_MODEL_KEY"));
  assert.ok(!body.includes("PRIVATE_SERVICE_TOKEN"));
  assert.equal(permittedPath("sessions/s1/model/settings", "POST"), false);
});

test("workspace groups retain separate absolute roots and prioritize the current workspace", () => {
  const sessions = [
    { session_id: "a", workspace_root: "/work/first/project" },
    { session_id: "b", workspace_root: "/work/second/project" },
    { session_id: "c", workspace_root: "/work/first/project" },
  ];
  const groups = groupWorkspaceSessions(sessions, "/work/second/project");
  assert.equal(groups.length, 3);
  assert.equal(groups[0].path, null);
  assert.equal(groups[1].path, "/work/second/project");
  assert.deepEqual(
    groups[1].sessions.map((item) => item.session_id),
    ["b"],
  );
  assert.deepEqual(
    groups[2].sessions.map((item) => item.session_id),
    ["a", "c"],
  );
  assert.equal(groups[1].name, "project");
  assert.equal(groups[2].name, "project");
  assert.equal(workspaceName("/"), "Root workspace");
  assert.equal(workspaceName(null), "Default workspace");
  assert.equal(workspaceName("/work/project/"), "project");
  assert.equal(groupWorkspaceSessions([], "/work/new")[1].path, "/work/new");
});

test("named workspaces persist in navigation before they have any chats", () => {
  const groups = groupWorkspaceSessions([], "/project/one", [
    { path: "/project/one", title: "First project" },
    { path: "/project/two", title: "Second project" },
  ]);
  assert.deepEqual(
    groups.map((item) => item.name),
    ["Default workspace", "First project", "Second project"],
  );
  assert.equal(groups[2].sessions.length, 0);
});

test("local directory chooser returns a real directory and cancellation creates nothing", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  const root = await mkdtemp(join(tmpdir(), "kyler-chooser-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const request = new Request("http://localhost:3002/api/workspaces/pick", {
    method: "POST",
    headers: { origin: "http://localhost:3002" },
  });
  const result = await chooseLocalDirectory(
    request,
    async (command, args) => {
      assert.equal(command, "/usr/bin/osascript");
      assert.equal(args[0], "-e");
      assert.match(args[1], /choose folder/);
      return { stdout: root + "/\n" };
    },
    "darwin",
  );
  assert.equal(result.status, 200);
  const chosen = await result.json();
  assert.ok(chosen.path.endsWith(root.split("/").at(-1)));
  assert.ok(chosen.name.startsWith("kyler-chooser-"));
  const cancelled = await chooseLocalDirectory(
    request,
    async () => {
      throw Object.assign(new Error("cancelled"), { code: 1, stderr: "User canceled. (-128)" });
    },
    "darwin",
  );
  assert.deepEqual(await cancelled.json(), { cancelled: true });
  let launched = false;
  const crossSite = await chooseLocalDirectory(
    new Request("http://localhost:3002/api/workspaces/pick", {
      method: "POST",
      headers: { origin: "https://untrusted.example" },
    }),
    async () => {
      launched = true;
      return { stdout: root };
    },
    "darwin",
  );
  assert.equal(crossSite.status, 403);
  assert.equal(launched, false);
  process.env.KYLER_CLIENT_SERVER_URL = "https://remote.example";
  const remote = await chooseLocalDirectory(
    request,
    async () => {
      launched = true;
      return { stdout: root };
    },
    "darwin",
  );
  assert.equal(remote.status, 409);
  assert.equal(launched, false);
});

test("draft adapter resolves its session only on send and publishes only after a reply", async (t) => {
  let admissions = 0;
  let published = 0;
  let resolved = 0;
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/runs")) return Response.json({ run_id: "r-draft" });
    if (path.endsWith("/events"))
      return stream([
        { seq: 1, type: "run_started" },
        { seq: 2, type: "text_delta", step: 1, text: "Hello" },
        { seq: 3, type: "text_delta", step: 1, text: " world" },
        { seq: 4, type: "run_completed", output: "Hello world" },
      ]);
    throw new Error(`Unexpected request ${path}`);
  });
  const adapter = chatAdapter(
    async () => {
      resolved++;
      return { session_id: "s-draft" };
    },
    () => {},
    undefined,
    {
      onAdmitted: () => {
        admissions++;
      },
      onResponse: () => {
        published++;
      },
    },
  );
  assert.equal(resolved, 0);
  assert.equal(published, 0);
  for await (const _ of adapter.run({
    messages: [{ role: "user", content: [{ type: "text", text: "Hi" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator) {
  }
  assert.equal(resolved, 1);
  assert.equal(admissions, 1);
  assert.equal(published, 1);
});

test("a rejected model request does not publish a draft conversation", async (t) => {
  let published = false;
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/runs")) return Response.json({ run_id: "r-rejected" });
    return stream([{ seq: 1, type: "run_failed", error: { message: "Model rejected request" } }]);
  });
  await assert.rejects(async () => {
    for await (const _ of chatAdapter("draft-session", () => {}, undefined, {
      onResponse: () => {
        published = true;
      },
    }).run({
      messages: [{ role: "user", content: [{ type: "text", text: "Hi" }], attachments: [] }],
      abortSignal: new AbortController().signal,
    } as never) as AsyncGenerator) {
    }
  }, /Model rejected/);
  assert.equal(published, false);
});

test("draft creation forwards its hidden history flag without adding arbitrary fields", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  t.mock.method(globalThis, "fetch", async (_url: URL, init: RequestInit) => {
    assert.deepEqual(JSON.parse(new TextDecoder().decode(init.body as Uint8Array)), {
      workspace_root: "/project",
      draft: true,
    });
    return Response.json({ session_id: "draft", draft: true }, { status: 201 });
  });
  const response = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/sessions", {
      method: "POST",
      body: JSON.stringify({ workspace_root: "/project", draft: true }),
    }),
    ["sessions"],
  );
  assert.equal(response.status, 201);
});

test("default workspace remains visible without creating a project", () => {
  assert.deepEqual(groupWorkspaceSessions([], null), [
    { path: null, name: "Default workspace", sessions: [] },
  ]);
  const projects = groupWorkspaceSessions([], null, [{ path: "/empty", title: "Empty project" }]);
  assert.equal(projects.length, 2);
  assert.equal(projects[0].path, null);
  assert.equal(projects[1].name, "Empty project");
});

test("global plugin browsing forwards without allocating a workspace or session", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  let calls = 0;
  t.mock.method(globalThis, "fetch", async (url: URL, init: RequestInit) => {
    assert.equal(url.pathname, "/v1/plugins/skills");
    assert.equal(init.method, "GET");
    calls++;
    return Response.json([{ id: "builtin", source: "builtin", name: "skill-creator" }]);
  });
  const response = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/plugins/skills"),
    ["plugins", "skills"],
  );
  assert.equal(response.status, 200);
  assert.equal(calls, 1);
  assert.equal((await response.json())[0].source, "builtin");
  assert.equal(permittedPath("plugins/connectors/test/connect", "POST"), true);
  assert.equal(permittedPath("plugins/connectors/upload", "POST"), true);
  assert.equal(permittedPath("plugins/connectors/test/execute", "POST"), false);
});

test("session titles prefer the server and use user input only as a placeholder", () => {
  assert.equal(sessionTitle("Server title", "User input"), "Server title");
  assert.equal(sessionTitle(null, "  User\n input  "), "User input");
  assert.equal(sessionTitle(" ", "Question"), "Question");
  assert.equal(sessionTitle(null, ""), "New chat");
});

test("default workspace is a real directory bound to the draft", () => {
  const groups = groupWorkspaceSessions([], "/default", [], "/default");
  assert.deepEqual(groups, [{ path: "/default", name: "Default workspace", sessions: [] }]);
});

test("whitespace-only model output never publishes a draft", async (t) => {
  let published = false;
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/runs")) return Response.json({ run_id: "whitespace" });
    return stream([
      { seq: 1, type: "text_delta", step: 1, text: " \n" },
      { seq: 2, type: "run_completed", output: " \n" },
    ]);
  });
  for await (const _ of chatAdapter("draft", () => {}, undefined, {
    onResponse: () => {
      published = true;
    },
  }).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Hi" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator) {
  }
  assert.equal(published, false);
});

test("execution rendering retains activity metadata and an authoritative final answer", async (t) => {
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/runs")) return Response.json({ run_id: "render" });
    if (path.includes("/tool-calls/"))
      return Response.json({ arguments: { path: "/project" }, output: "files" });
    return stream([
      { seq: 1, type: "run_started", timestamp_ms: 1000 },
      { seq: 2, type: "text_delta", step: 1, text: "Inspecting files" },
      {
        seq: 3,
        type: "tool_started",
        call_id: "read",
        name: "list_dir",
        activity: "explore",
        summary: "Listed project files",
        timestamp_ms: 2000,
        source: { kind: "mcp", server_id: "project" },
      },
      { seq: 4, type: "tool_finished", call_id: "read", status: "success", timestamp_ms: 4000 },
      { seq: 5, type: "run_completed", output: "Final answer", timestamp_ms: 5000 },
    ]);
  });
  let final: ChatModelRunResult | undefined;
  for await (const update of chatAdapter("session", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Inspect" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    final = update;
  assert.equal(final?.content?.at(-1)?.type, "text");
  assert.deepEqual(final?.content?.at(-1), { type: "text", text: "Final answer" });
  const tool = final?.content?.[1];
  assert.equal(tool?.type, "tool-call");
  if (tool?.type === "tool-call") {
    assert.deepEqual(tool.args, { activity: "explore", summary: "Listed project files" });
    assert.deepEqual(tool.providerMetadata?.kyler, {
      batch: 1,
      activity: "explore",
      summary: "Listed project files",
      startedAt: 2000,
      finishedAt: 4000,
      sourceLabel: "MCP · project",
    });
  }
  assert.deepEqual(final?.metadata?.custom?.kylerRun, {
    startedAt: 1000,
    finishedAt: 5000,
  });
});

test("default drafts defer workspace selection to the server and preserve its authoritative info", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_WORKSPACE = "/ignored-ui-workspace";
  t.mock.method(globalThis, "fetch", async (url: URL, init: RequestInit) => {
    if (url.pathname === "/v1/info")
      return Response.json({ model: "fixture", default_workspace: "/server/config/workspace" });
    assert.deepEqual(JSON.parse(new TextDecoder().decode(init.body as Uint8Array)), {
      draft: true,
    });
    return Response.json({
      session_id: "draft",
      workspace_root: "/server/config/workspace",
      draft: true,
    });
  });
  const info = await proxyRequest(new Request("http://localhost/api/kyler/info"), ["info"]);
  assert.equal((await info.json()).default_workspace, "/server/config/workspace");
  const created = await proxyRequest(
    new Request("http://localhost/api/kyler/sessions", {
      method: "POST",
      body: JSON.stringify({ draft: true }),
    }),
    ["sessions"],
  );
  assert.equal((await created.json()).workspace_root, "/server/config/workspace");
});

test("background drafts stay reachable while running and empty drafts stay hidden", () => {
  const local = [
    { session_id: "empty", draft: true, status: "idle", placeholder: "" },
    { session_id: "waiting", draft: true, status: "running", placeholder: "Build a report" },
    { session_id: "failed", draft: true, status: "idle", placeholder: "No reply" },
    { session_id: "answered", draft: false, status: "idle", placeholder: "Hello" },
  ];
  assert.deepEqual(
    mergeSessionHistory([], local).map((item) => item.session_id),
    ["waiting", "answered"],
  );
});

test("history uses server titles, deduplicates sessions and keeps background activity", () => {
  const saved = [{ session_id: "a", title: "Server title", status: "completed" }];
  const local = [{ session_id: "a", title: "Old title", placeholder: "Prompt", status: "running" }];
  assert.deepEqual(mergeSessionHistory(saved, local), [
    {
      session_id: "a",
      title: "Server title",
      placeholder: "Prompt",
      status: "running",
    },
  ]);
});

test("tool batch metadata follows step events even when a step contains no text", async (t) => {
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/runs")) return Response.json({ run_id: "batch-run" });
    if (path.endsWith("/events"))
      return stream([
        { seq: 1, type: "step_started", step: 1 },
        { seq: 2, type: "tool_started", call_id: "a", name: "read_file" },
        { seq: 3, type: "tool_started", call_id: "b", name: "list_dir" },
        { seq: 4, type: "step_started", step: 2 },
        { seq: 5, type: "tool_started", call_id: "c", name: "run_command" },
        { seq: 6, type: "run_completed", output: "Done" },
      ]);
    throw new Error(`Unexpected request: ${path}`);
  });
  let final: ChatModelRunResult | undefined;
  for await (const update of chatAdapter("batches", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Inspect" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    final = update;
  assert.deepEqual(
    final?.content
      ?.filter((part) => part.type === "tool-call")
      .map((part) => part.providerMetadata?.kyler?.batch),
    [1, 1, 2],
  );
});

test("skill ZIP upload proxy forwards binary bytes and permits skill removal", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  process.env.KYLER_CLIENT_TOKEN = "upload-test-token";
  const bytes = new Uint8Array([80, 75, 3, 4, 0, 255, 128]);
  t.mock.method(globalThis, "fetch", async (url: URL, init?: RequestInit) => {
    assert.equal(url.pathname, "/v1/plugins/skills/upload");
    assert.equal(new Headers(init?.headers).get("content-type"), "application/zip");
    assert.deepEqual(new Uint8Array(await new Response(init?.body).arrayBuffer()), bytes);
    return Response.json({ name: "fixture", source: "user" }, { status: 201 });
  });
  const response = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/plugins/skills/upload", {
      method: "POST",
      headers: { "content-type": "application/zip", origin: "http://localhost:3002" },
      body: bytes,
    }),
    ["plugins", "skills", "upload"],
  );
  assert.equal(response.status, 201);
  assert.equal(permittedPath("plugins/skills/fixture", "DELETE"), true);
});

test("skill uninstall forwards its fingerprint and surfaces a stale-version rejection", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  process.env.KYLER_CLIENT_TOKEN = "uninstall-test-token";
  t.mock.method(globalThis, "fetch", async (url: URL, init?: RequestInit) => {
    assert.equal(url.pathname, "/v1/plugins/skills/abc123");
    assert.equal(init?.method, "DELETE");
    assert.deepEqual(JSON.parse(await new Response(init?.body).text()), {
      fingerprint: "version-1",
    });
    return Response.json(
      { code: "invalid_request", message: "Skill changed. Reload before removing." },
      { status: 400 },
    );
  });
  const response = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/plugins/skills/abc123", {
      method: "DELETE",
      headers: { "content-type": "application/json", origin: "http://localhost:3002" },
      body: JSON.stringify({ fingerprint: "version-1" }),
    }),
    ["plugins", "skills", "abc123"],
  );
  assert.equal(response.status, 400);
  assert.match(JSON.stringify(await response.json()), /Skill changed/);
});

test("an accepted empty MCP action allows the connector status refresh to continue", async (t) => {
  const paths: string[] = [];
  t.mock.method(globalThis, "fetch", async (path: string) => {
    paths.push(path);
    if (path.endsWith("/connect")) return new Response(null, { status: 202 });
    return Response.json([{ id: "fixture", status: "connecting" }]);
  });
  assert.equal(
    await api("plugins/connectors/fixture/connect", { method: "POST", body: "{}" }),
    undefined,
  );
  assert.deepEqual(await api("plugins/connectors"), [{ id: "fixture", status: "connecting" }]);
  assert.deepEqual(paths, [
    "/api/kyler/plugins/connectors/fixture/connect",
    "/api/kyler/plugins/connectors",
  ]);
});

test("successful empty responses are supported and malformed nonempty JSON remains an error", async (t) => {
  for (const [status, body] of [
    [200, ""],
    [202, "  \n"],
    [204, null],
    [205, null],
  ] as const) {
    t.mock.method(globalThis, "fetch", async () => new Response(body, { status }));
    assert.equal(await api("empty"), undefined);
  }
  t.mock.method(globalThis, "fetch", async () => new Response("{invalid", { status: 200 }));
  await assert.rejects(api("invalid"), /Kyler returned invalid JSON \(200\)/);
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({ message: "Connection rejected" }, { status: 400 }),
  );
  await assert.rejects(api("rejected"), /Connection rejected/);
});

test("connector uninstall proxy forwards DELETE and accepts an empty success response", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  process.env.KYLER_CLIENT_TOKEN = "connector-test-token";
  t.mock.method(globalThis, "fetch", async (url: URL, init?: RequestInit) => {
    assert.equal(url.pathname, "/v1/plugins/connectors/fixture");
    assert.equal(init?.method, "DELETE");
    return new Response(null, { status: 204 });
  });
  const response = await proxyRequest(
    new Request("http://localhost:3002/api/kyler/plugins/connectors/fixture", {
      method: "DELETE",
      headers: { origin: "http://localhost:3002" },
    }),
    ["plugins", "connectors", "fixture"],
  );
  assert.equal(response.status, 204);
  assert.equal(await response.text(), "");
});

test("artifact downloads preserve binary bytes and download headers", async (t) => {
  const previous = { ...process.env };
  t.after(() => {
    process.env = previous;
  });
  process.env.KYLER_CONFIG = join(tmpdir(), `kyler-no-config-${crypto.randomUUID()}.toml`);
  process.env.KYLER_CLIENT_SERVER_URL = "http://127.0.0.1:30001";
  const bytes = new Uint8Array([0, 255, 1, 128]);
  t.mock.method(
    globalThis,
    "fetch",
    async () =>
      new Response(bytes, {
        headers: {
          "content-type": "application/octet-stream",
          "content-disposition": 'attachment; filename="report.pptx"',
        },
      }),
  );
  const response = await proxyRequest(
    new Request("http://localhost/api/kyler/sessions/s1/artifacts/a1/content"),
    ["sessions", "s1", "artifacts", "a1", "content"],
  );
  assert.deepEqual(new Uint8Array(await response.arrayBuffer()), bytes);
  assert.equal(response.headers.get("content-disposition"), 'attachment; filename="report.pptx"');
  assert.equal(response.headers.get("x-content-type-options"), "nosniff");
  assert.equal(permittedPath("sessions/s1/artifacts", "POST"), false);
  assert.equal(permittedPath("artifacts/a1/content", "GET"), false);
});

test("truncated completion retrieves full output and retains execution identity", async (t) => {
  const output = "Done: [Report](./output/report.pptx)";
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/runs")) return Response.json({ run_id: "r1" });
    if (path.endsWith("/events"))
      return stream([
        { seq: 1, type: "text_delta", step: 1, text: "Done:", execution_id: "e1" },
        {
          seq: 2,
          type: "run_completed",
          output: "Done:",
          output_truncated: true,
          execution_id: "e1",
        },
      ]);
    if (path.endsWith("/runs/r1")) return Response.json({ output });
    throw new Error("Unexpected request");
  });
  let result: ChatModelRunResult | undefined;
  for await (const update of chatAdapter("s1", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Make report" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    result = update;
  assert.deepEqual(result?.content, [{ type: "text", text: output }]);
  assert.equal(result?.metadata?.custom?.executionId, "e1");
});

test("tool completion never waits for a details endpoint before subsequent events render", async (t) => {
  let requestedDetails = false;
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.includes("tool-calls")) {
      requestedDetails = true;
      throw new Error("Details temporarily unavailable");
    }
    if (path.endsWith("/runs")) return Response.json({ run_id: "progress" });
    return stream([
      { seq: 1, type: "step_started", step: 1 },
      { seq: 2, type: "tool_started", call_id: "a", name: "read_file", activity: "explore" },
      { seq: 3, type: "tool_finished", call_id: "a", status: "success" },
      { seq: 4, type: "step_started", step: 2 },
      { seq: 5, type: "tool_started", call_id: "b", name: "read_file", activity: "explore" },
      {
        seq: 6,
        type: "tool_finished",
        call_id: "b",
        status: "error",
        error: { message: "File not found" },
      },
      { seq: 7, type: "text_delta", step: 3, text: "Finished" },
      { seq: 8, type: "run_completed", output: "Finished" },
    ]);
  });
  const updates: ChatModelRunResult[] = [];
  for await (const update of chatAdapter("session", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Inspect" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>)
    updates.push(update);
  assert.equal(requestedDetails, false);
  assert.ok(
    updates.some(
      (update) =>
        update.content?.[0]?.type === "tool-call" && update.content[0].result === "success",
    ),
  );
  const final = updates.at(-1)!;
  assert.equal(final.content?.length, 3);
  const failed = final.content?.[1];
  assert.ok(failed?.type === "tool-call");
  assert.equal(failed.isError, true);
  assert.equal(failed.result, "File not found");
  assert.equal(final.status?.type, "complete");
});

test("a clarification request publishes the conversation before the user answers", async (t) => {
  let published = 0;
  let questionUpdates = 0;
  const question = {
    id: "question",
    session_id: "session",
    execution_id: "execution",
    tool_call_id: "call",
    questions: [],
    status: "pending",
    answers: {},
    created_at: 100,
    resolved_at: null,
  };
  t.mock.method(globalThis, "fetch", async (path: string) => {
    if (path.endsWith("/runs")) return Response.json({ run_id: "question-run" });
    return stream([
      { seq: 1, type: "question_updated", question },
      { seq: 2, type: "question_updated", question: { ...question, status: "answered" } },
      { seq: 3, type: "run_completed", output: "Done" },
    ]);
  });
  for await (const _ of chatAdapter("session", () => {}, undefined, {
    onResponse: () => {
      published++;
    },
    onQuestion: () => {
      questionUpdates++;
    },
  }).run({
    messages: [
      { role: "user", content: [{ type: "text", text: "Create a report" }], attachments: [] },
    ],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator) {
  }
  assert.equal(published, 1);
  assert.equal(questionUpdates, 2);
});

test("successful plan tools hydrate steps during streaming before the run finishes", async (t) => {
  const first = {
    summary: "Inspect and build",
    steps: [
      { id: "inspect", content: "Inspect files", status: "completed" },
      { id: "build", content: "Build report", status: "in_progress" },
    ],
  };
  const complete = {
    ...first,
    steps: first.steps.map((step) => ({ ...step, status: "completed" })),
  };
  const calls: string[] = [];
  t.mock.method(globalThis, "fetch", async (path: string) => {
    calls.push(path);
    if (path.endsWith("/runs")) return Response.json({ run_id: "plan-run" });
    if (path.endsWith("/events"))
      return stream([
        {
          seq: 1,
          type: "tool_started",
          call_id: "p1",
          name: "update_plan",
          summary: "Update plan",
        },
        { seq: 2, type: "tool_finished", call_id: "p1", status: "success" },
        { seq: 3, type: "tool_started", call_id: "failed", name: "update_plan" },
        { seq: 4, type: "tool_finished", call_id: "failed", status: "error" },
        { seq: 5, type: "tool_started", call_id: "p2", name: "update_plan" },
        { seq: 6, type: "tool_finished", call_id: "p2", status: "success" },
        { seq: 7, type: "run_completed", output: "Done" },
      ]);
    if (path.endsWith("/tool-calls/p1"))
      return Response.json({ name: "update_plan", status: "success", arguments: first });
    if (path.endsWith("/tool-calls/p2"))
      return Response.json({ name: "update_plan", status: "success", arguments: complete });
    throw new Error(`Unexpected request: ${path}`);
  });
  const updates = chatAdapter("s1", () => {}).run({
    messages: [{ role: "user", content: [{ type: "text", text: "Build" }], attachments: [] }],
    abortSignal: new AbortController().signal,
  } as never) as AsyncGenerator<ChatModelRunResult>;
  const snapshots = [];
  for await (const update of updates)
    snapshots.push({
      status: update.status?.type,
      plan: latestDisplayPlan([{ role: "assistant", content: update.content ?? [] }]),
    });
  assert.equal(snapshots[0].plan, undefined);
  assert.ok(
    snapshots.some(
      (snapshot) =>
        snapshot.status !== "complete" && snapshot.plan?.steps[1].status === "in_progress",
    ),
  );
  assert.deepEqual(snapshots.at(-1)?.plan, complete);
  assert.equal(calls.filter((path) => path.includes("/tool-calls/")).length, 2);
});
