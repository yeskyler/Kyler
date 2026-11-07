import { api } from "./client.ts";
import type { Session } from "./types.ts";

export function createDraftSession(workspace?: string | null) {
  return api<Session>("sessions", {
    method: "POST",
    body: JSON.stringify({ draft: true, ...(workspace ? { workspace_root: workspace } : {}) }),
  });
}

export async function discardDraft(id: string) {
  try {
    const session = await api<Session>(`sessions/${id}`);
    if (session.draft && !["starting", "running", "cancelling"].includes(session.status ?? ""))
      await api(`sessions/${id}`, { method: "DELETE" });
  } catch {
    // Cleanup is best-effort; active or already-published sessions are retained.
  }
}
