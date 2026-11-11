import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { parse } from "smol-toml";

export async function readConfig() {
  const file = process.env.KYLER_CONFIG ?? join(homedir(), ".kyler", "config.toml");
  let config: Record<string, unknown> = {};
  try {
    config = parse(await readFile(file, "utf8"));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") {
      throw new Error("Cannot read Kyler configuration. Check KYLER_CONFIG.");
    }
  }
  return config;
}

export async function serverSettings() {
  const config = await readConfig();
  const client = config.client as Record<string, unknown> | undefined;
  const server = config.server as Record<string, unknown> | undefined;
  const url = new URL(
    String(
      process.env.KYLER_CLIENT_SERVER_URL ??
        process.env.KYLER_SERVER_URL ??
        client?.server_url ??
        "http://127.0.0.1:30001",
    ),
  );
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) {
    throw new Error("Kyler server URL must be HTTP or HTTPS without embedded credentials.");
  }
  const local = new URL(`http://${String(server?.bind ?? "127.0.0.1:30001")}`);
  if (local.hostname === "0.0.0.0") local.hostname = "127.0.0.1";
  if (local.hostname === "[::]") local.hostname = "[::1]";
  const sameBackend =
    url.href === local.href ||
    (url.protocol === "http:" &&
      url.hostname === "localhost" &&
      url.port === local.port &&
      url.pathname === "/" &&
      ["127.0.0.1", "[::1]"].includes(local.hostname));
  const token =
    process.env.KYLER_CLIENT_TOKEN ??
    process.env.KYLER_SERVER_TOKEN ??
    (client?.access_token || (sameBackend ? server?.access_token : ""));
  return {
    url,
    token: typeof token === "string" ? token : "",
  };
}

// Only expose UI operations. Service shutdown and arbitrary upstream URLs are excluded.
export function permittedPath(path: string, method: string) {
  const id = "[a-zA-Z0-9_-]+";
  const rules: [string, string][] = [
    ["GET", "info"],
    ["GET", "workspaces"],
    ["POST", "workspaces"],
    ["DELETE", `workspaces/${id}`],
    ["GET", "plugins/skills"],
    ["POST", "plugins/skills"],
    ["POST", "plugins/skills/reload"],
    ["POST", "plugins/skills/upload"],
    ["GET", `plugins/skills/${id}`],
    ["DELETE", `plugins/skills/${id}`],
    ["POST", `plugins/skills/${id}/(?:enable|disable)`],
    ["GET", "plugins/connectors"],
    ["POST", "plugins/connectors"],
    ["POST", "plugins/connectors/upload"],
    ["DELETE", `plugins/connectors/${id}`],
    ["POST", `plugins/connectors/${id}/(?:connect|disconnect|reconnect)`],
    ["GET", "models/settings"],
    ["PUT", "models/settings"],
    ["POST", "sessions"],
    ["GET", "sessions"],
    ["GET", "models"],
    ["POST", "models/reload"],
    ["GET", `sessions/${id}`],
    ["GET", `sessions/${id}/rollout`],
    ["GET", `sessions/${id}/rollout/[0-9]+`],
    ["GET", `sessions/${id}/context`],
    ["GET", `sessions/${id}/artifacts`],
    ["GET", `sessions/${id}/workspace-files`],
    ["GET", `sessions/${id}/artifacts/${id}/content`],
    ["GET", `sessions/${id}/memory/inspect`],
    ["DELETE", `sessions/${id}`],
    ["GET", `sessions/${id}/messages`],
    ["GET", `sessions/${id}/history`],
    ["GET", `sessions/${id}/questions`],
    ["POST", `sessions/${id}/questions/${id}/response`],
    ["GET", `sessions/${id}/model`],
    ["PUT", `sessions/${id}/model`],
    ["GET", `sessions/${id}/model/settings`],
    ["PUT", `sessions/${id}/model/settings`],
    ["GET", `sessions/${id}/skills`],
    ["POST", `sessions/${id}/skills`],
    ["POST", `sessions/${id}/skills/reload`],
    ["GET", `sessions/${id}/skills/${id}`],
    ["POST", `sessions/${id}/skills/${id}/(?:enable|disable)`],
    ["GET", `sessions/${id}/mcp`],
    ["POST", `sessions/${id}/mcp`],
    ["POST", `sessions/${id}/mcp/${id}/(?:connect|disconnect|reconnect)`],
    ["GET", `sessions/${id}/approvals`],
    ["GET", `sessions/${id}/permissions`],
    ["PUT", `sessions/${id}/permissions`],
    ["POST", `sessions/${id}/runs`],
    ["POST", `sessions/${id}/attachments`],
    ["GET", `sessions/${id}/attachments`],
    ["GET", `sessions/${id}/attachments/${id}`],
    ["GET", `sessions/${id}/attachments/${id}/content`],
    ["DELETE", `sessions/${id}/attachments/${id}`],
    ["GET", `sessions/${id}/tool-calls/${id}`],
    ["GET", `runs/${id}`],
    ["GET", `runs/${id}/events`],
    ["POST", `runs/${id}/cancel`],
    ["POST", `approvals/${id}/decision`],
  ];
  return rules.some(([verb, pattern]) => verb === method && new RegExp(`^${pattern}$`).test(path));
}

export async function proxyRequest(request: Request, path: string[]) {
  const route = path.join("/");
  if (!permittedPath(route, request.method)) {
    return Response.json({ message: "Unsupported Kyler operation." }, { status: 404 });
  }
  const origin = request.headers.get("origin");
  // Next.js may canonicalize the internal URL to localhost; Host retains the
  // browser destination and cannot be changed by a cross-site fetch.
  const destination = new URL(request.url);
  destination.host = request.headers.get("host") ?? destination.host;
  if (request.headers.get("x-forwarded-proto")?.split(",")[0].trim() === "https")
    destination.protocol = "https:";
  if (
    (origin && origin !== destination.origin) ||
    request.headers.get("sec-fetch-site") === "cross-site"
  ) {
    return Response.json({ message: "Cross-origin requests are not permitted." }, { status: 403 });
  }
  try {
    const settings = await serverSettings();
    const target = new URL(`/v1/${route}`, settings.url);
    target.search = new URL(request.url).search;
    const headers = new Headers();
    if (settings.token) headers.set("authorization", `Bearer ${settings.token}`);
    for (const name of ["content-type", "last-event-id"]) {
      const value = request.headers.get(name);
      if (value) headers.set(name, value);
    }
    let body: Uint8Array | undefined;
    if (!["GET", "HEAD"].includes(request.method)) {
      const reader = request.body?.getReader();
      const chunks: Uint8Array[] = [];
      let size = 0;
      if (reader) {
        try {
          while (true) {
            const next = await reader.read();
            if (next.done) break;
            size += next.value.byteLength;
            if (size > 20 * 1024 * 1024) {
              await reader.cancel();
              return Response.json({ message: "Upload exceeds 20 MB." }, { status: 413 });
            }
            chunks.push(next.value);
          }
        } finally {
          reader.releaseLock();
        }
      }
      body = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) {
        body.set(chunk, offset);
        offset += chunk.length;
      }
      if (route === "sessions") {
        let requested: { workspace_root?: unknown; draft?: unknown; workspace_mode?: unknown };
        try {
          requested = body.length ? JSON.parse(new TextDecoder().decode(body)) : {};
        } catch {
          return Response.json({ message: "Invalid session request." }, { status: 400 });
        }
        if (
          !requested ||
          typeof requested !== "object" ||
          Array.isArray(requested) ||
          Object.keys(requested).some(
            (key) => key !== "workspace_root" && key !== "draft" && key !== "workspace_mode",
          )
        )
          return Response.json({ message: "Invalid session request." }, { status: 400 });
        if (requested.draft !== undefined && typeof requested.draft !== "boolean")
          return Response.json({ message: "Invalid draft flag." }, { status: 400 });
        if (
          requested.workspace_mode !== undefined &&
          requested.workspace_mode !== "project" &&
          requested.workspace_mode !== "isolated"
        )
          return Response.json({ message: "Invalid workspace mode." }, { status: 400 });
        const workspace = requested.workspace_root;
        if (
          workspace != null &&
          (typeof workspace !== "string" ||
            !workspace.startsWith("/") ||
            workspace.length > 4096 ||
            workspace.includes("\0"))
        )
          return Response.json(
            { message: "An absolute working directory is required." },
            { status: 400 },
          );
        body = new TextEncoder().encode(
          JSON.stringify({
            ...(typeof workspace === "string" ? { workspace_root: workspace } : {}),
            ...(requested.draft === undefined ? {} : { draft: requested.draft }),
            ...(requested.workspace_mode === undefined
              ? {}
              : { workspace_mode: requested.workspace_mode }),
          }),
        );
        headers.set("content-type", "application/json");
      }
    }
    const response = await fetch(target, {
      method: request.method,
      headers,
      body: body as BodyInit | undefined,
      signal: request.signal,
      cache: "no-store",
      redirect: "error",
    });
    return new Response(response.body, {
      status: response.status,
      headers: {
        "content-type": response.headers.get("content-type") ?? "application/json",
        "cache-control": "no-store, no-transform",
        ...(response.headers.has("content-disposition")
          ? { "content-disposition": response.headers.get("content-disposition")! }
          : {}),
        "x-content-type-options": "nosniff",
        "x-accel-buffering": "no",
      },
    });
  } catch {
    return Response.json(
      { message: "Cannot connect to Kyler. Start the service and check the server configuration." },
      { status: 502 },
    );
  }
}
