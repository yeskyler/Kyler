import { logout } from "@/lib/kyler/web-auth";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export function POST(request: Request) {
  return logout(request);
}
