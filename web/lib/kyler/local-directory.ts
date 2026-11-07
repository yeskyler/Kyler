import { execFile } from "node:child_process";
import { realpath, stat } from "node:fs/promises";
import { basename } from "node:path";
import { promisify } from "node:util";
import { serverSettings } from "./server.ts";

const execute = promisify(execFile);
export type DirectoryLauncher = (command: string, args: string[]) => Promise<{ stdout: string }>;

export async function chooseLocalDirectory(
  request: Request,
  launch: DirectoryLauncher = (command, args) =>
    execute(command, args, { timeout: 120_000, maxBuffer: 16_384 }),
  platform = process.platform,
) {
  const destination = new URL(request.url);
  destination.host = request.headers.get("host") ?? destination.host;
  const origin = request.headers.get("origin");
  if (
    (origin && origin !== destination.origin) ||
    request.headers.get("sec-fetch-site") === "cross-site"
  )
    return Response.json({ message: "Cross-origin requests are not permitted." }, { status: 403 });
  if (!["localhost", "127.0.0.1", "[::1]"].includes(destination.hostname))
    return Response.json(
      { message: "Use localhost to choose a local directory." },
      { status: 403 },
    );
  const settings = await serverSettings();
  if (!["localhost", "127.0.0.1", "[::1]"].includes(settings.url.hostname))
    return Response.json(
      { message: "Local directory selection requires Kyler to run on this computer." },
      { status: 409 },
    );
  try {
    let result: { stdout: string };
    if (platform === "darwin")
      result = await launch("/usr/bin/osascript", [
        "-e",
        'POSIX path of (choose folder with prompt "Choose a Kyler project directory")',
      ]);
    else if (platform === "linux")
      result = await launch("/usr/bin/zenity", [
        "--file-selection",
        "--directory",
        "--title=Choose a Kyler project directory",
      ]);
    else
      return Response.json(
        { message: "A native directory chooser is unavailable on this platform." },
        { status: 501 },
      );
    const selected = result.stdout.trim();
    if (!selected) return Response.json({ cancelled: true });
    const path = await realpath(selected);
    if (!(await stat(path)).isDirectory())
      return Response.json({ message: "Select a directory." }, { status: 400 });
    return Response.json({ path, name: basename(path) || "Workspace" });
  } catch (error) {
    const details = error as { code?: number; stderr?: string; killed?: boolean };
    if (
      !details.killed &&
      details.code === 1 &&
      (!details.stderr?.trim() || details.stderr.includes("-128"))
    )
      return Response.json({ cancelled: true });
    return Response.json(
      { message: "The directory chooser could not open. Allow desktop access and try again." },
      { status: 503 },
    );
  }
}
