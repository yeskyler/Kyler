import { api, KylerApiError } from "./client.ts";
import type { Artifact } from "./artifacts.ts";
import type { AttachmentInfo, WorkspaceInput } from "./types.ts";

export type WorkspaceFiles = { inputs: WorkspaceInput[]; outputs: Artifact[] };

/** Older services still support session files; unavailable aggregation must not hide them. */
export async function loadWorkspaceFiles(
  session: string,
  signal: AbortSignal,
): Promise<WorkspaceFiles> {
  try {
    const files = await api<WorkspaceFiles>(`sessions/${session}/workspace-files`, { signal });
    if (!files || !Array.isArray(files.inputs) || !Array.isArray(files.outputs))
      throw new Error("Invalid workspace file response.");
    return {
      inputs: files.inputs.filter((file) => file.session_id === session),
      outputs: files.outputs.filter((file) => file.session_id === session),
    };
  } catch (error) {
    signal.throwIfAborted();
    if (!(error instanceof KylerApiError) || ![404, 405, 501].includes(error.status)) throw error;
  }
  const [outputs, inputs] = await Promise.allSettled([
    api<Artifact[]>(`sessions/${session}/artifacts`, { signal }),
    api<AttachmentInfo[]>(`sessions/${session}/attachments`, { signal }),
  ]);
  signal.throwIfAborted();
  if (outputs.status === "rejected" && inputs.status === "rejected") throw outputs.reason;
  return {
    outputs: outputs.status === "fulfilled" ? outputs.value : [],
    inputs:
      inputs.status === "fulfilled"
        ? inputs.value.map((attachment) => ({ session_id: session, attachment }))
        : [],
  };
}
