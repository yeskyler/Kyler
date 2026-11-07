import { chooseLocalDirectory } from "@/lib/kyler/local-directory";
export const runtime = "nodejs";
export async function POST(request: Request) {
  try {
    return await chooseLocalDirectory(request);
  } catch {
    return Response.json(
      { message: "Cannot load the local Kyler configuration." },
      { status: 503 },
    );
  }
}
