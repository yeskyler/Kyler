import { login } from "@/lib/kyler/web-auth";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  try {
    return await login(request);
  } catch {
    return Response.json({ message: "Cannot read Web access configuration." }, { status: 503 });
  }
}
