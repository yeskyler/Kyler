import { proxyRequest } from "@/lib/kyler/server";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

async function proxy(request: Request, context: { params: Promise<{ path: string[] }> }) {
  return proxyRequest(request, (await context.params).path);
}

export { proxy as GET, proxy as POST, proxy as PUT, proxy as DELETE };
