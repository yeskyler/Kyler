import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import {
  installPreviewStorage,
  previewStorageKey,
  storageSnapshot,
} from "../lib/kyler/preview-storage.ts";

test("sandboxed games can read and write artifact storage without accessing host storage", () => {
  const window: Record<string, Storage> = {};
  for (const key of ["localStorage", "sessionStorage"])
    Object.defineProperty(window, key, {
      get: () => {
        throw new DOMException("Opaque origin", "SecurityError");
      },
      configurable: true,
    });
  const messages: { type: string; channel: string; value: Record<string, string> }[] = [];
  runInNewContext(`(${installPreviewStorage.toString()})({best:"42"},"artifact-channel")`, {
    window,
    parent: { postMessage: (value: (typeof messages)[number]) => messages.push(value) },
    DOMException,
  });
  assert.equal(window.localStorage.getItem("best"), "42");
  window.localStorage.setItem("score", 123 as unknown as string);
  assert.equal(window.localStorage.getItem("score"), "123");
  window.localStorage.highScore = "200";
  assert.equal(window.localStorage.getItem("highScore"), "200");
  assert.equal(window.localStorage.length, 3);
  assert.deepEqual(Object.keys(window.localStorage), ["best", "score", "highScore"]);
  delete window.localStorage.highScore;
  window.localStorage.removeItem("score");
  assert.equal(window.localStorage.getItem("score"), null);
  const previous = messages.length;
  window.sessionStorage.setItem("temporary", "yes");
  assert.equal(window.sessionStorage.getItem("temporary"), "yes");
  assert.equal(messages.length, previous);
  assert.equal(messages.at(-1)?.type, "kyler-preview-storage");
  assert.equal(messages.at(-1)?.channel, "artifact-channel");
  assert.throws(() => window.localStorage.setItem("large", "x".repeat(65537)), {
    name: "QuotaExceededError",
  });
  window.localStorage.clear();
  assert.equal(window.localStorage.length, 0);
});
test("storage persistence validates payloads and separates sessions and files", () => {
  assert.equal(storageSnapshot({ key: 1 }), null);
  assert.equal(storageSnapshot(["value"]), null);
  assert.equal(storageSnapshot({ key: "x".repeat(65537) }), null);
  assert.deepEqual(storageSnapshot({ key: "value" }), { key: "value" });
  assert.notEqual(previewStorageKey("session-a", "file"), previewStorageKey("session-b", "file"));
  assert.notEqual(
    previewStorageKey("session-a", "file-a"),
    previewStorageKey("session-a", "file-b"),
  );
});
