import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { readConfig } from "./server.ts";

export const sessionCookie = "kyler_web_session";
const sessionSeconds = 7 * 24 * 60 * 60;
const equal = (left: string, right: string) =>
  timingSafeEqual(
    createHash("sha256").update(left).digest(),
    createHash("sha256").update(right).digest(),
  );
const sign = (payload: string, token: string) =>
  createHmac("sha256", token).update(`kyler-web:${payload}`).digest("base64url");

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

export function createSession(token: string, now = Date.now()): string {
  const payload = `v1.${Math.floor(now / 1000) + sessionSeconds}.${randomBytes(16).toString("hex")}`;
  return `${payload}.${sign(payload, token)}`;
}
export function validSession(value: string, token: string, now = Date.now()): boolean {
  const match = value.match(/^(v1\.([0-9]{1,12})\.[a-f0-9]{32})\.([A-Za-z0-9_-]{43})$/);
  if (!match || !token) return false;
  const expires = Number(match[2]);
  const seconds = Math.floor(now / 1000);
  return (
    expires > seconds &&
    expires <= seconds + sessionSeconds &&
    equal(match[3], sign(match[1], token))
  );
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
export function checkWebAccess(request: Request, configuration: Record<string, unknown>) {
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
  if (validSession(cookieValue(request), token)) return null;
  if (url.pathname.startsWith("/api/"))
    return Response.json(
      { message: "Sign in with your Web token." },
      { status: 401, headers: { "cache-control": "no-store" } },
    );
  const login = new URL("/login", url);
  login.searchParams.set("next", safeNext(url.pathname + url.search));
  return new Response(null, {
    status: 303,
    headers: { location: login.href, "cache-control": "no-store" },
  });
}

export async function login(request: Request, configuration?: Record<string, unknown>) {
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
  if (typeof supplied !== "string" || supplied.length > 256 || !equal(supplied.trim(), token))
    return Response.json(
      { message: "Invalid Web token." },
      { status: 401, headers: { "cache-control": "no-store" } },
    );
  return Response.json(
    { ok: true },
    {
      headers: {
        "set-cookie": cookieHeader(createSession(token), request),
        "cache-control": "no-store",
      },
    },
  );
}
export function logout(request: Request) {
  if (!sameOrigin(request))
    return Response.json({ message: "Cross-origin sign-out is not permitted." }, { status: 403 });
  return Response.json(
    { ok: true },
    { headers: { "set-cookie": cookieHeader("", request, true), "cache-control": "no-store" } },
  );
}
export async function authorizeWeb(request: Request) {
  return checkWebAccess(request, await webConfiguration());
}
