import assert from "node:assert/strict";
import test from "node:test";
import { checkWebAccess } from "../lib/kyler/web-auth.ts";

test("loopback defaults stay usable and public bindings fail closed without a password", () => {
  const request = new Request("http://localhost:30002/");
  assert.equal(checkWebAccess(request, {}), null);
  assert.equal(checkWebAccess(request, {}, "0.0.0.0:30002")?.status, 503);
  assert.equal(checkWebAccess(request, {}, "[::]:30002")?.status, 503);
  assert.equal(checkWebAccess(request, {}, "[::1]:30002"), null);
});

test("configured Web passwords protect both pages and APIs", () => {
  const configuration = { web: { bind: "0.0.0.0:30002", access_token: "private-web-password" } };
  for (const path of ["/", "/api/kyler/sessions", "/_next/static/example.js"]) {
    assert.equal(
      checkWebAccess(new Request(`http://localhost:30002${path}`), configuration)?.status,
      401,
    );
    const authorization = `Basic ${Buffer.from("kyler:private-web-password").toString("base64")}`;
    assert.equal(
      checkWebAccess(
        new Request(`http://localhost:30002${path}`, { headers: { authorization } }),
        configuration,
      ),
      null,
    );
  }
  assert.equal(
    checkWebAccess(
      new Request("http://localhost:30002/", { headers: { authorization: "Basic invalid" } }),
      configuration,
    )?.status,
    401,
  );
});
