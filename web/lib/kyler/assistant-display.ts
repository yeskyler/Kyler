import type { ThreadAssistantMessagePart } from "@assistant-ui/react";

/** Keep the final answer visible while collecting prior work in chronological order. */
export function partitionAssistantParts(parts: readonly ThreadAssistantMessagePart[]) {
  let lastActivity = -1;
  let lastText = -1;
  parts.forEach((part, index) => {
    if (part.type === "tool-call" || part.type === "reasoning") lastActivity = index;
    if (part.type === "text" && part.text.trim()) lastText = index;
  });
  const finalStart = lastText > lastActivity ? lastText : parts.length;
  const process: number[] = [];
  const answer: number[] = [];
  parts.forEach((part, index) => {
    if (part.type === "text" && !part.text.trim()) return;
    (index < finalStart ? process : answer).push(index);
  });
  return { process, answer };
}

export function formatRunDuration(milliseconds: number) {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}m ${seconds % 60}s`;
}

/** Consecutive calls share one visual tree; batch metadata stays on each call. */
export function groupProcessParts(
  parts: readonly ThreadAssistantMessagePart[],
  indices: readonly number[],
) {
  const rows: { kind: "tool" | "content"; indices: number[] }[] = [];
  for (const index of indices) {
    const part = parts[index];
    const row = rows.at(-1);
    if (part?.type === "tool-call" && row?.kind === "tool") row.indices.push(index);
    else rows.push({ kind: part?.type === "tool-call" ? "tool" : "content", indices: [index] });
  }
  return rows;
}

export function activityCategory(value: unknown) {
  switch (value) {
    case "explore":
      return "Explore";
    case "execute":
      return "Execute";
    case "edit":
      return "Edit";
    case "search":
      return "Search";
    case "think":
      return "Think";
    default:
      return "Tool";
  }
}

/** A parallel batch has one branch per category, in first-appearance order. */
export function groupToolCategories(
  parts: readonly ThreadAssistantMessagePart[],
  indices: readonly number[],
) {
  const groups: { category: string; indices: number[] }[] = [];
  for (const index of indices) {
    const part = parts[index];
    if (part?.type !== "tool-call") continue;
    const category = activityCategory(part.providerMetadata?.kyler?.activity);
    const previous = groups.find((group) => group.category === category);
    if (previous) previous.indices.push(index);
    else groups.push({ category, indices: [index] });
  }
  return groups;
}

export type DisplayPlan = {
  summary: string;
  steps: { id: string; content: string; status: "pending" | "in_progress" | "completed" }[];
};

/** Only committed tool results replace the displayed plan, including history replay. */
export function latestDisplayPlan(
  messages: readonly { role: string; content: readonly unknown[] }[],
): DisplayPlan | undefined {
  for (const message of [...messages].reverse()) {
    if (message.role !== "assistant") continue;
    for (const value of [...message.content].reverse()) {
      if (!value || typeof value !== "object") continue;
      const part = value as Record<string, unknown>;
      if (
        part.type !== "tool-call" ||
        part.toolName !== "update_plan" ||
        part.isError ||
        part.result === undefined
      )
        continue;
      const args = part.args as Partial<DisplayPlan> | undefined;
      if (!args || typeof args.summary !== "string" || !Array.isArray(args.steps)) continue;
      if (
        args.steps.every(
          (step) =>
            step &&
            typeof step.id === "string" &&
            typeof step.content === "string" &&
            ["pending", "in_progress", "completed"].includes(step.status),
        )
      )
        return { summary: args.summary, steps: args.steps };
    }
  }
}
