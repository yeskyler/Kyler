import { timingSafeEqual, createHash } from "node:crypto";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

// Management uses a separate per-launch secret. Never expose a PID/identity to unauthenticated callers.
export function GET(request: Request) {
  const secret = process.env.KYLER_WEB_SERVICE_SECRET;
  const id = process.env.KYLER_WEB_INSTANCE_ID;
  const supplied = request.headers.get("authorization") ?? "";
  const hash = (value: string) => createHash("sha256").update(value).digest();
  if (!secret || !id || !timingSafeEqual(hash(supplied), hash(`Bearer ${secret}`)))
    return Response.json({ message: "Unauthorized." }, { status: 401 });
  return Response.json(
    { instance_id: id, pid: process.pid },
    { headers: { "cache-control": "no-store" } },
  );
}
