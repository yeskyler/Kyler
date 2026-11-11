import assert from "node:assert/strict";
import test from "node:test";
import { navigateSession, sessionFromUrl, sessionUrl } from "../lib/kyler/session-navigation.ts";

test("session links preserve unrelated query parameters and fragments", () => {
  const original = "http://localhost:30002/?panel=files#output";
  const selected = sessionUrl(original, "session-1");
  assert.equal(selected, "/chat/session-1?panel=files#output");
  assert.equal(sessionFromUrl(new URL(selected, original).href), "session-1");
  assert.equal(sessionUrl(new URL(selected, original).href, null), "/?panel=files#output");
  assert.equal(sessionFromUrl(original), null);
  assert.equal(sessionFromUrl("http://localhost/chat/"), null);
  assert.equal(sessionFromUrl("http://localhost/chat/first/extra"), null);
  assert.equal(sessionFromUrl("http://localhost/chat/%ZZ"), null);
  assert.equal(sessionFromUrl("http://localhost/chat/first/"), "first");
  assert.equal(sessionUrl("http://localhost/?session=old", "new"), "/chat/new");
});

test("session identifiers are encoded as a single path segment", () => {
  const id = "one/two?three&four#five";
  const url = new URL(sessionUrl("http://localhost/", id), "http://localhost");
  assert.equal(url.pathname, `/chat/${encodeURIComponent(id)}`);
  assert.equal(sessionFromUrl(url.href), id);
});

test("switching pushes history, draft admission replaces it, and restoration adds no entries", (t) => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  const entries = ["http://localhost:30002/"];
  let pushes = 0;
  let replacements = 0;
  const browser = {
    get location() {
      return new URL(entries.at(-1)!);
    },
    history: {
      pushState(_state: unknown, _title: string, url: string) {
        pushes++;
        entries.push(new URL(url, entries.at(-1)).href);
      },
      replaceState(_state: unknown, _title: string, url: string) {
        replacements++;
        entries[entries.length - 1] = new URL(url, entries.at(-1)).href;
      },
    },
  };
  Object.defineProperty(globalThis, "window", { configurable: true, value: browser });
  t.after(() => {
    if (previous) Object.defineProperty(globalThis, "window", previous);
    else Reflect.deleteProperty(globalThis, "window");
  });
  navigateSession("first", "push");
  navigateSession("first", "push");
  navigateSession("second", "push");
  assert.equal(pushes, 2);
  assert.equal(sessionFromUrl(browser.location.href), "second");
  navigateSession(null, "push");
  navigateSession("new-chat", "replace");
  assert.equal(pushes, 3);
  assert.equal(replacements, 1);
  assert.equal(sessionFromUrl(entries[entries.length - 2]), "second");
  // The browser has already changed the URL before delivering popstate.
  entries.pop();
  navigateSession("second", "none");
  assert.equal(sessionFromUrl(browser.location.href), "second");
  assert.equal(pushes, 3);
  assert.equal(replacements, 1);
});
