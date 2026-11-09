import type { ThreadAssistantMessagePart } from "@assistant-ui/react";

export type Compaction = {
  id: string;
  status: "running" | "completed" | "failed" | "cancelled" | "interrupted";
  started_at: number;
  finished_at: number | null;
};
export function isCompactionPart(part?: ThreadAssistantMessagePart) {
  return part?.type === "data" && part.name === "context-compaction";
}
export function compactionPart(record: Compaction): ThreadAssistantMessagePart {
  return { type: "data", id: `compaction:${record.id}`, name: "context-compaction", data: record };
}
export function upsertCompaction(parts: ThreadAssistantMessagePart[], record: Compaction) {
  if (
    !record ||
    typeof record.id !== "string" ||
    !record.id ||
    record.id.length > 256 ||
    !["running", "completed", "failed", "cancelled", "interrupted"].includes(record.status) ||
    !Number.isFinite(record.started_at)
  )
    return;
  const index = parts.findIndex(
    (part) => isCompactionPart(part) && part.type === "data" && part.data.id === record.id,
  );
  const previous = parts[index];
  if (previous?.type === "data" && previous.data.status !== "running") return;
  if (index < 0) parts.push(compactionPart(record));
  else parts[index] = compactionPart(record);
}
export function settleCompactions(
  parts: ThreadAssistantMessagePart[],
  status: "cancelled" | "interrupted",
) {
  parts.forEach((part, index) => {
    if (isCompactionPart(part) && part.type === "data" && part.data.status === "running")
      parts[index] = compactionPart({ ...part.data, status });
  });
}

/** Separators keep their chronological position and never enter collapsible execution details. */
export function executionSections(
  parts: readonly ThreadAssistantMessagePart[],
  indices: readonly number[],
) {
  const sections: { kind: "details" | "compaction"; indices: number[] }[] = [];
  for (const index of indices) {
    if (isCompactionPart(parts[index])) sections.push({ kind: "compaction", indices: [index] });
    else if (sections.at(-1)?.kind === "details") sections.at(-1)!.indices.push(index);
    else sections.push({ kind: "details", indices: [index] });
  }
  return sections;
}
