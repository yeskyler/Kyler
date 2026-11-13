export type PatchFileChange = {
  path: string;
  operation: "added" | "modified" | "deleted" | "renamed";
  previous_path?: string;
  added: number;
  removed: number;
};
export type PatchResult = {
  kind: "patch_result";
  status: "success" | "partial" | "error";
  files: PatchFileChange[];
  error?: string;
};

/** Decode the core's committed results, never infer edits from model arguments. */
export function parsePatchResult(value: unknown): PatchResult | undefined {
  try {
    const result = typeof value === "string" ? JSON.parse(value) : value;
    if (
      !result ||
      typeof result !== "object" ||
      result.kind !== "patch_result" ||
      !["success", "partial", "error"].includes(result.status) ||
      !Array.isArray(result.files) ||
      (result.error !== undefined && typeof result.error !== "string")
    )
      return;
    if (
      !result.files.every(
        (file: PatchFileChange) =>
          file &&
          typeof file.path === "string" &&
          ["added", "modified", "deleted", "renamed"].includes(file.operation) &&
          Number.isSafeInteger(file.added) &&
          file.added >= 0 &&
          Number.isSafeInteger(file.removed) &&
          file.removed >= 0 &&
          (file.previous_path === undefined || typeof file.previous_path === "string"),
      )
    )
      return;
    return result;
  } catch {
    return;
  }
}

/** Only confirmed live results from this user turn belong in the composer strip. */
export function latestComposerPatch(
  messages: readonly { id?: string; role: string; content: readonly unknown[] }[],
): { key: string; result: PatchResult } | undefined {
  const userIndex = messages.findLastIndex((message) => message.role === "user");
  for (let index = messages.length - 1; index > userIndex; index--) {
    const message = messages[index];
    if (message.role !== "assistant") continue;
    for (const value of [...message.content].reverse()) {
      if (!value || typeof value !== "object") continue;
      const part = value as Record<string, unknown>;
      if (part.type !== "tool-call" || part.toolName !== "apply_patch" || part.result === undefined)
        continue;
      const metadata = part.providerMetadata as { kyler?: Record<string, unknown> } | undefined;
      const data = metadata?.kyler;
      // Replayed history stays in tool nodes, not the newly entered composer.
      if (
        data?.historyOutput !== undefined ||
        ["preparing", "queued", "approval", "running"].includes(String(data?.state))
      )
        continue;
      const result = parsePatchResult(data?.patch ?? part.result);
      if (!result || result.status === "error" || result.files.length === 0) continue;
      return { key: `patch:${JSON.stringify([message.id ?? index, part.toolCallId])}`, result };
    }
  }
}
