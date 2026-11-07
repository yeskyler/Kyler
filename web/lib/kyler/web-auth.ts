import { createHash, timingSafeEqual } from "node:crypto";
import { readConfig } from "./server.ts";

const loopback = (value: string) =>
  /^(127\.\d+\.\d+\.\d+|localhost)(:\d+)?$/.test(value) || /^\[?::1\]?(?::\d+)?$/.test(value);

export function checkWebAccess(
  request: Request,
  configuration: Record<string, unknown>,
  bind?: string,
) {
  const web = configuration.web as Record<string, unknown> | undefined;
  const token = typeof web?.access_token === "string" ? web.access_token : "";
  const address = bind ?? String(web?.bind ?? "127.0.0.1:30002");
  if (!token) {
    if (!loopback(address))
      return Response.json(
        { message: "Configure a Web access password before using a non-loopback address." },
        { status: 503 },
      );
    return null;
  }
  const authorization = request.headers.get("authorization") ?? "";
  let password = "";
  if (authorization.startsWith("Basic ") && authorization.length < 8192) {
    const decoded = Buffer.from(authorization.slice(6), "base64").toString("utf8");
    const separator = decoded.indexOf(":");
    if (separator >= 0) password = decoded.slice(separator + 1);
  }
  const digest = (value: string) => createHash("sha256").update(value).digest();
  if (timingSafeEqual(digest(password), digest(token))) return null;
  return new Response("Web access requires authentication.", {
    status: 401,
    headers: {
      "WWW-Authenticate": 'Basic realm="Kyler", charset="UTF-8"',
      "Cache-Control": "no-store",
    },
  });
}

export async function authorizeWeb(request: Request) {
  const configuration = await readConfig();
  if (process.env.KYLER_WEB_TOKEN !== undefined) {
    configuration.web = {
      ...(configuration.web as Record<string, unknown> | undefined),
      access_token: process.env.KYLER_WEB_TOKEN,
    };
  }
  return checkWebAccess(request, configuration, process.env.KYLER_WEB_BIND);
}
