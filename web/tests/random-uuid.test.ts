import assert from "node:assert/strict";
import test from "node:test";
import { randomUuid } from "../lib/kyler/random-uuid.ts";
import { chatAdapter } from "../lib/kyler/client.ts";
import { pastedTextDocument } from "../lib/kyler/pasted-document.ts";

const uuidV4 = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;

test("native UUID generation is preferred and keeps its Crypto receiver", () => {
  const native = "0b775a13-965a-4e67-a317-2123a55fe93a";
  const source = {
    randomUUID() {
      assert.equal(this, source);
      return native;
    },
    getRandomValues() {
      throw new Error("Fallback must not run");
    },
  };
  assert.equal(randomUuid(source), native);
});

test("fallback creates RFC 4122 v4 UUIDs using fresh secure random bytes", () => {
  let calls = 0;
  const source = {
    getRandomValues(bytes: Uint8Array) {
      assert.equal(this, source);
      assert.equal(bytes.length, 16);
      for (let index = 0; index < bytes.length; index++) bytes[index] = index + calls;
      calls++;
      return bytes;
    },
  };
  const first = randomUuid(source);
  const second = randomUuid(source);
  assert.equal(first, "00010203-0405-4607-8809-0a0b0c0d0e0f");
  assert.match(first, uuidV4);
  assert.match(second, uuidV4);
  assert.notEqual(first, second);
  assert.equal(calls, 2);
});

test("missing secure randomness fails explicitly instead of using Math.random", () => {
  assert.throws(() => randomUuid({}), /Secure random generation is unavailable/);
});

test("message admission, SSE replay and pasted documents work without crypto.randomUUID", async (t) => {
  const original = globalThis.crypto;
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "crypto")!;
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: { getRandomValues: original.getRandomValues.bind(original) },
  });
  t.after(() => Object.defineProperty(globalThis, "crypto", descriptor));
  assert.equal(typeof globalThis.crypto.randomUUID, "undefined");
  let admissions = 0;
  let streams = 0;
  const ids = new Set<string>();
  t.mock.method(globalThis, "fetch", async (path: string, init?: RequestInit) => {
    if (path === "/api/kyler/sessions/session-1/runs") {
      const body = JSON.parse(String(init?.body));
      assert.match(body.client_request_id, uuidV4);
      ids.add(body.client_request_id);
      admissions++;
      return Response.json({ run_id: "run-1" });
    }
    assert.equal(path, "/api/kyler/runs/run-1/events");
    streams++;
    const reconnect = streams % 2 === 0;
    assert.equal(new Headers(init?.headers).get("last-event-id"), reconnect ? "1" : "0");
    const events = [{ seq: 1, type: "text_delta", step: 1, text: "Answer" }];
    const replay = reconnect
      ? [...events, { seq: 2, type: "run_completed", output: "Answer" }]
      : events;
    return new Response(replay.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""), {
      headers: { "content-type": "text/event-stream" },
    });
  });
  for (let turn = 0; turn < 2; turn++) {
    const adapter = chatAdapter("session-1", () => {});
    for await (const _ of adapter.run({
      messages: [{ role: "user", content: [{ type: "text", text: "Hello" }], attachments: [] }],
      abortSignal: new AbortController().signal,
    } as never) as AsyncIterable<unknown>) {
      /* Consume the simulated model stream. */
    }
  }
  assert.equal(admissions, 2);
  assert.equal(streams, 4);
  assert.equal(ids.size, 2);
  const file = pastedTextDocument("中文 text", new Date("2026-01-01T00:00:00Z"));
  assert.match(file.name, /^pasted-text-2026-01-01T00-00-00-000Z-[a-f0-9]{8}\.docx$/);
  assert.ok(file.size > 0);
});
