export type MemoryStatus = "candidate" | "active" | "rejected" | "invalid";
export type MemoryEntry = {
  key: string;
  content: string;
  tags: string[];
  revision: number;
  updated_at: number;
  source_session: string;
  source_execution: string | null;
  source_tool_call: string | null;
  reviewed_at: number | null;
  status: MemoryStatus;
  metadata: {
    kind: "fact" | "preference" | "lesson" | "procedure";
    conditions: string;
    evidence: string[];
    expires_at: number | null;
    sensitive: boolean;
  };
};
export type MemoryPage = {
  scope: string;
  entries: MemoryEntry[];
  total: number;
  next_cursor: number | null;
};
export type MemoryDetails = { scope: string; entry: MemoryEntry };
export function canActivateMemory(entry: MemoryEntry, now = Date.now()): boolean {
  return (
    !entry.metadata.sensitive &&
    (entry.metadata.expires_at === null || entry.metadata.expires_at > now)
  );
}
