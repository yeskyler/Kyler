import { createHash, timingSafeEqual } from "node:crypto";
import { configPath, readConfig } from "./server.ts";
import { resolve } from "node:path";
import { sessionSeconds, WebSessions } from "./web-sessions.ts";

export const sessionCookie = "kyler_web_session";
const attemptWindow = 60_000;
const maxFailures = 5;
let attempts = { scope: "", failures: 0, until: 0 };
const sessions = () => new WebSessions(`${resolve(configPath())}.web-sessions`);
const equal = (left: string, right: string) =>
  timingSafeEqual(
    createHash("sha256").update(left).digest(),
    createHash("sha256").update(right).digest(),
  );
export function safeNext(value: unknown): string {
  if (
    typeof value !== "string" ||
    !value ||
    !value.startsWith("/") ||
    value.startsWith("//") ||
    [...value].some((character) => character === "\\" || character.charCodeAt(0) <= 32)
  )
    return "/";
  const url = new URL(value, "http://kyler.local");
  if (
    url.origin !== "http://kyler.local" ||
    url.pathname === "/login" ||
    url.pathname.startsWith("/api/")
  )
    return "/";
  return `${url.pathname}${url.search}${url.hash}`;
}

function cookieValue(request: Request): string {
  return (
    request.headers
      .get("cookie")
      ?.split(";")
      .map((value) => value.trim())
      .find((value) => value.startsWith(`${sessionCookie}=`))
      ?.slice(sessionCookie.length + 1) ?? ""
  );
}
function secure(request: Request) {
  return (
    new URL(request.url).protocol === "https:" ||
    request.headers.get("x-forwarded-proto")?.split(",")[0].trim() === "https"
  );
}
export function cookieHeader(value: string, request: Request, clear = false): string {
  return `${sessionCookie}=${value}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${clear ? 0 : sessionSeconds}${secure(request) ? "; Secure" : ""}`;
}
function sameOrigin(request: Request): boolean {
  const destination = new URL(request.url);
  destination.host = request.headers.get("host") ?? destination.host;
  if (secure(request)) destination.protocol = "https:";
  return (
    request.headers.get("origin") === destination.origin &&
    request.headers.get("sec-fetch-site") !== "cross-site"
  );
}
export async function webConfiguration() {
  const config = await readConfig();
  if (process.env.KYLER_WEB_TOKEN !== undefined) {
    config.web = {
      ...(config.web as Record<string, unknown> | undefined),
      access_token: process.env.KYLER_WEB_TOKEN,
    };
  }
  return config;
}
function webToken(configuration: Record<string, unknown>): string {
  const token = (configuration.web as Record<string, unknown> | undefined)?.access_token;
  return typeof token === "string" ? token : "";
}
function publicPath(path: string): boolean {
  return (
    path === "/login" ||
    path === "/api/auth/login" ||
    path === "/api/auth/logout" ||
    path.startsWith("/_next/static/") ||
    path.startsWith("/_next/webpack-hmr") ||
    path === "/icon.svg" ||
    path === "/favicon.ico"
  );
}
export async function checkWebAccess(
  request: Request,
  configuration: Record<string, unknown>,
  store = sessions(),
) {
  const url = new URL(request.url);
  url.host = request.headers.get("host") ?? url.host;
  if (secure(request)) url.protocol = "https:";
  if (publicPath(url.pathname)) return null;
  const token = webToken(configuration);
  if (!token)
    return Response.json(
      { message: "Web token is not configured. Run kyler configure --web." },
      { status: 503, headers: { "cache-control": "no-store" } },
    );
  if (await store.valid(cookieValue(request), token)) return null;
  if (url.pathname.startsWith("/api/"))
    return Response.json(
      { message: "Your Web session is missing or expired. Sign in again." },
      { status: 401, headers: { "cache-control": "no-store" } },
    );
  const login = new URL("/login", url);
  login.searchParams.set("next", safeNext(url.pathname + url.search));
  return new Response(null, {
    status: 303,
    headers: { location: login.href, "cache-control": "no-store" },
  });
}

export async function login(
  request: Request,
  configuration?: Record<string, unknown>,
  store = sessions(),
  now = Date.now(),
) {
  if (!sameOrigin(request))
    return Response.json({ message: "Cross-origin sign-in is not permitted." }, { status: 403 });
  const token = webToken(configuration ?? (await webConfiguration()));
  if (!token)
    return Response.json(
      { message: "Web token is not configured. Run kyler configure --web." },
      { status: 503 },
    );
  if (!request.headers.get("content-type")?.startsWith("application/json"))
    return Response.json({ message: "Expected a JSON token request." }, { status: 400 });
  let supplied: unknown;
  const reader = request.body?.getReader();
  let body = "";
  let size = 0;
  const decoder = new TextDecoder();
  try {
    if (reader) {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > 4096) {
          await reader.cancel();
          return Response.json({ message: "Token request is too large." }, { status: 413 });
        }
        body += decoder.decode(value, { stream: true });
      }
      body += decoder.decode();
    }
    supplied = JSON.parse(body).token;
  } catch {
    return Response.json({ message: "Invalid token request." }, { status: 400 });
  } finally {
    reader?.releaseLock();
  }
  const scope = resolve(configPath());
  if (attempts.scope !== scope || now >= attempts.until)
    attempts = { scope, failures: 0, until: now + attemptWindow };
  if (attempts.failures >= maxFailures)
    return Response.json(
      { message: "Too many sign-in attempts. Try again shortly." },
      {
        status: 429,
        headers: {
          "retry-after": String(Math.ceil((attempts.until - now) / 1000)),
          "cache-control": "no-store",
        },
      },
    );
  if (typeof supplied !== "string" || supplied.length > 256 || !equal(supplied.trim(), token)) {
    attempts.failures++;
    return Response.json(
      { message: "Invalid Web token." },
      { status: 401, headers: { "cache-control": "no-store" } },
    );
  }
  attempts.failures = 0;
  const id = await store.create(token, now);
  await store.revoke(cookieValue(request));
  return Response.json(
    { ok: true },
    {
      headers: {
        "set-cookie": cookieHeader(id, request),
        "cache-control": "no-store",
      },
    },
  );
}
export async function logout(request: Request, store = sessions()) {
  if (!sameOrigin(request))
    return Response.json({ message: "Cross-origin sign-out is not permitted." }, { status: 403 });
  await store.revoke(cookieValue(request));
  return Response.json(
    { ok: true },
    { headers: { "set-cookie": cookieHeader("", request, true), "cache-control": "no-store" } },
  );
}
export async function authorizeWeb(request: Request) {
  return checkWebAccess(request, await webConfiguration());
}

/** Keep browser authority attached to an upstream event stream until it closes. */
export async function proxyWebEvents(
  request: Request,
  open: (request: Request) => Promise<Response>,
  check = authorizeWeb,
  interval = 2000,
) {
  const denied = await check(request);
  if (denied) return denied;
  const revoked = new AbortController();
  const signal = AbortSignal.any([request.signal, revoked.signal]);
  let pending = false;
  let timer: ReturnType<typeof setInterval>;
  const clear = () => clearInterval(timer);
  timer = setInterval(async () => {
    if (pending) return;
    pending = true;
    try {
      if (await check(request)) revoked.abort();
    } catch {
      revoked.abort();
    } finally {
      pending = false;
    }
  }, interval);
  signal.addEventListener("abort", clear, { once: true });
  try {
    const response = await open(new Request(request, { signal }));
    if (signal.aborted || !response.ok || !response.body) {
      clear();
      return response;
    }
    const upstream = response.body.getReader();
    const closeUpstream = () => void upstream.cancel().catch(() => {});
    signal.addEventListener("abort", closeUpstream, { once: true });
    if (signal.aborted) closeUpstream();
    const stop = () => {
      clear();
      signal.removeEventListener("abort", clear);
      signal.removeEventListener("abort", closeUpstream);
    };
    return new Response(
      new ReadableStream({
        async pull(controller) {
          try {
            const { done, value } = await upstream.read();
            if (done || signal.aborted) {
              stop();
              controller.close();
            } else {
              controller.enqueue(value);
            }
          } catch (error) {
            stop();
            if (signal.aborted) controller.close();
            else controller.error(error);
          }
        },
        async cancel(reason) {
          stop();
          await upstream.cancel(reason);
        },
      }),
      { status: response.status, headers: response.headers },
    );
  } catch (error) {
    clear();
    throw error;
  }
}
