import { proxyWebEvents } from "@/lib/kyler/web-auth";
import { proxyRequest } from "@/lib/kyler/server";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

async function proxy(request: Request, context: { params: Promise<{ path: string[] }> }) {
  const { path } = await context.params;
  if (request.method === "GET" && path.length === 3 && path[0] === "runs" && path[2] === "events")
    return proxyWebEvents(request, (authorized) => proxyRequest(authorized, path));
  return proxyRequest(request, path);
}

export { proxy as GET, proxy as POST, proxy as PUT, proxy as DELETE };
