import assert from "node:assert/strict";
import test from "node:test";
import {
  checkWebAccess,
  createSession,
  validSession,
  login,
  logout,
  cookieHeader,
  safeNext,
  sessionCookie,
} from "../lib/kyler/web-auth.ts";

const token = "synthetic-web-token-for-tests";
const configuration = { web: { bind: "0.0.0.0:30002", access_token: token } };
const signIn = (origin = "http://localhost:30002", value = token) =>
  new Request("http://localhost:30002/api/auth/login", {
    method: "POST",
    headers: { origin, "content-type": "application/json" },
    body: JSON.stringify({ token: value }),
  });

test("local and network pages require token sessions while login assets stay public", () => {
  for (const host of ["localhost:30002", "192.168.1.10:30002"]) {
    const page = checkWebAccess(new Request(`http://${host}/chat/s1`), configuration)!;
    assert.equal(page.status, 303);
    const redirect = new URL(page.headers.get("location")!);
    assert.equal(redirect.pathname, "/login");
    assert.equal(redirect.searchParams.get("next"), "/chat/s1");
    assert.equal(
      checkWebAccess(new Request(`http://${host}/api/kyler/sessions`), configuration)?.status,
      401,
    );
  }
  for (const path of ["/login", "/_next/static/example.js", "/icon.svg"])
    assert.equal(checkWebAccess(new Request(`http://localhost${path}`), {}), null);
  assert.equal(checkWebAccess(new Request("http://localhost/"), {})?.status, 503);
});

test("token login issues a signed HttpOnly cookie without placing the token in it", async () => {
  const response = await login(signIn(), configuration);
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { ok: true });
  const cookie = response.headers.get("set-cookie")!;
  assert.ok(cookie.includes("HttpOnly"));
  assert.ok(cookie.includes("SameSite=Lax"));
  assert.ok(!cookie.includes("Secure"), "HTTP LAN login must work");
  assert.ok(!cookie.includes(token));
  const authenticated = new Request("http://localhost:30002/chat/s1", {
    headers: { cookie: cookie.split(";")[0] },
  });
  assert.equal(checkWebAccess(authenticated, configuration), null);
  assert.equal(
    checkWebAccess(authenticated, { web: { bind: "127.0.0.1:30002", access_token: token } }),
    null,
  );
  assert.equal(
    checkWebAccess(authenticated, { web: { access_token: "rotated-token" } })?.status,
    303,
  );
});

test("token login checks browser Host when Next uses an internal localhost URL", async () => {
  const request = new Request("http://localhost:30002/api/auth/login", {
    method: "POST",
    headers: {
      host: "192.168.1.10:30002",
      origin: "http://192.168.1.10:30002",
      "content-type": "application/json",
    },
    body: JSON.stringify({ token }),
  });
  assert.equal((await login(request, configuration)).status, 200);
});

test("invalid tokens and cross-site or opaque origins never create sessions", async () => {
  for (const origin of ["null", "https://evil.example", "http://localhost:30003"])
    assert.equal((await login(signIn(origin), configuration)).status, 403);
  const rejected = await login(signIn("http://localhost:30002", "incorrect-token"), configuration);
  assert.equal(rejected.status, 401);
  assert.equal(rejected.headers.get("set-cookie"), null);
  assert.ok(!(await rejected.text()).includes(token));
  assert.equal((await login(signIn(), {})).status, 503);
  const basic = `Basic ${Buffer.from(`kyler:${token}`).toString("base64")}`;
  assert.equal(
    checkWebAccess(
      new Request("http://localhost/api/kyler/info", { headers: { authorization: basic } }),
      configuration,
    )?.status,
    401,
  );
});

test("signed sessions expire and reject tampering and reset tokens", () => {
  const now = 1_700_000_000_000;
  const session = createSession(token, now);
  assert.ok(validSession(session, token, now + 1000));
  assert.ok(!validSession(session, token, now + 7 * 24 * 60 * 60 * 1000));
  assert.ok(!validSession(session + "x", token, now));
  assert.ok(!validSession(session.replace("v1.", "v2."), token, now));
  assert.ok(!validSession(session, "different-token", now));
  assert.ok(!validSession(session, "", now));
  assert.ok(!validSession("malformed", token, now));
});

test("HTTPS cookies are secure and logout clears the session with origin protection", () => {
  assert.ok(cookieHeader("value", new Request("https://kyler.example/")).includes("; Secure"));
  const proxied = new Request("http://localhost/api/auth/logout", {
    method: "POST",
    headers: {
      host: "kyler.example",
      origin: "https://kyler.example",
      "x-forwarded-proto": "https",
    },
  });
  const response = logout(proxied);
  assert.equal(response.status, 200);
  assert.ok(response.headers.get("set-cookie")!.includes(`${sessionCookie}=;`));
  assert.ok(response.headers.get("set-cookie")!.includes("Max-Age=0"));
  assert.ok(response.headers.get("set-cookie")!.includes("; Secure"));
  assert.equal(
    logout(
      new Request("http://localhost/api/auth/logout", {
        method: "POST",
        headers: { origin: "https://evil.example" },
      }),
    ).status,
    403,
  );
});

test("login bodies are bounded and malformed requests do not authenticate", async () => {
  const request = (body: string) =>
    new Request("http://localhost:30002/api/auth/login", {
      method: "POST",
      headers: { origin: "http://localhost:30002", "content-type": "application/json" },
      body,
    });
  assert.equal((await login(request("invalid"), configuration)).status, 400);
  assert.equal((await login(request("null"), configuration)).status, 400);
  assert.equal(
    (await login(request(JSON.stringify({ token: "x".repeat(5000) })), configuration)).status,
    413,
  );
});

test("post-login destinations remain local and reject redirect loops", () => {
  assert.equal(safeNext("/chat/s1?panel=files#output"), "/chat/s1?panel=files#output");
  for (const value of [
    null,
    ["/chat/s1"],
    "https://evil.example",
    "//evil.example",
    "/\\evil.example",
    "/login?next=/login",
    "/api/kyler/info",
    "/\ninvalid",
  ])
    assert.equal(safeNext(value), "/");
});
