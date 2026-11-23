import type { ThreadAssistantMessagePart } from "@assistant-ui/react";

export type SubagentPresentation = {
  task_id: string;
  name: string;
  agent: "general" | "review" | "memory";
  status: "queued" | "running" | "completed" | "failed" | "timed_out" | "cancelled";
  target?:
    | { type: "uncommitted" }
    | { type: "base_branch"; branch: string }
    | { type: "commit"; commit: string }
    | null;
  summary?: string | null;
  error?: string | null;
};

export const isSubagentPart = (part?: ThreadAssistantMessagePart) =>
  part?.type === "data" && part.name === "subagent-task";

export function upsertSubagent(parts: ThreadAssistantMessagePart[], task: SubagentPresentation) {
  const part: ThreadAssistantMessagePart = {
    type: "data",
    id: `subagent:${task.task_id}`,
    name: "subagent-task",
    data: task,
  };
  const index = parts.findIndex(
    (item) => item.type === "data" && isSubagentPart(item) && item.id === part.id,
  );
  if (index === -1) parts.push(part);
  else parts[index] = part;
}

export function isSubagentTool(name: string) {
  return ["delegate_task", "wait_tasks", "cancel_task", "get_task"].includes(name);
}
