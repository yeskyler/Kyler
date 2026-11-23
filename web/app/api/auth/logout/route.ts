import { logout } from "@/lib/kyler/web-auth";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  try {
    return await logout(request);
  } catch {
    return Response.json({ message: "Could not end the Web session. Try again." }, { status: 503 });
  }
}
