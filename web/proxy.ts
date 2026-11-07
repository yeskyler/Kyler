import { NextResponse, type NextRequest } from "next/server";
import { authorizeWeb } from "@/lib/kyler/web-auth";

export async function proxy(request: NextRequest) {
  if (request.nextUrl.pathname === "/api/service/identity") return NextResponse.next();
  try {
    return (await authorizeWeb(request)) ?? NextResponse.next();
  } catch {
    return NextResponse.json(
      { message: "Cannot read Web service configuration." },
      { status: 503 },
    );
  }
}

export const config = { matcher: "/:path*" };
