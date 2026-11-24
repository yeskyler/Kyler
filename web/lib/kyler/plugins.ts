import type { Connector, Skill } from "./types.ts";

export const skillSources = { builtin: "System", user: "Personal", project: "Project" } as const;
export type SkillCategory = "system" | "user";
export function skillCategory(skill: Pick<Skill, "source">): SkillCategory {
  return skill.source === "builtin" ? "system" : "user";
}

export const pluginStatuses: Record<string, string> = {
  enabled: "Enabled",
  disabled: "Disabled",
  needs_approval: "Needs review",
  shadowed: "Overridden",
  invalid: "Invalid",
  connected: "Connected",
  disconnected: "Not connected",
  connecting: "Connecting",
  failed: "Connection failed",
};
export function filterSkills(items: Skill[], query: string, source: string, status: string) {
  const term = query.trim().toLocaleLowerCase();
  return items.filter(
    (item) =>
      (source === "all" || item.source === source) &&
      (status === "all" || item.status === status) &&
      (!term ||
        `${item.name} ${item.display_name ?? ""} ${item.description}`
          .toLocaleLowerCase()
          .includes(term)),
  );
}
export function filterConnectors(
  items: Connector[],
  query: string,
  source: string,
  status: string,
) {
  const term = query.trim().toLocaleLowerCase();
  return items.filter(
    (item) =>
      (source === "all" || (source === "custom" ? item.removable : !item.removable)) &&
      (status === "all" || item.status === status) &&
      (!term ||
        item.id.toLocaleLowerCase().includes(term) ||
        item.tools.some((tool) =>
          `${tool.name} ${tool.description}`.toLocaleLowerCase().includes(term),
        )),
  );
}

export type SkillDraft = {
  name: string;
  description: string;
  fingerprint: string;
  resources: string[];
};
export type DraftPage = {
  scope: string;
  drafts: SkillDraft[];
  total: number;
  next_cursor: number | null;
};
export type DraftDetails = {
  scope: string;
  draft: { info: SkillDraft; files: Record<string, string> };
  installed_fingerprint: string | null;
  review_status: "pending" | "installed" | "discarded";
};
export function draftFilesReviewed(
  files: Record<string, string>,
  visited: readonly string[],
): boolean {
  const names = Object.keys(files);
  return names.length > 0 && names.every((name) => visited.includes(name));
}

/** Only successful memory tool results can propose a review card. Never parse assistant prose. */
export function skillProposal(part: {
  type: string;
  toolName?: string;
  isError?: boolean;
  result?: unknown;
  providerMetadata?: { kyler?: { historyOutput?: unknown } };
}): SkillDraft | undefined {
  if (part.type !== "tool-call" || part.toolName !== "memory" || part.isError) return;
  const raw = part.providerMetadata?.kyler?.historyOutput ?? part.result;
  try {
    const value = typeof raw === "string" ? JSON.parse(raw) : raw;
    const draft = value?.draft;
    if (
      !draft ||
      typeof draft.name !== "string" ||
      !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(draft.name) ||
      typeof draft.fingerprint !== "string" ||
      !/^[a-f0-9]{64}$/.test(draft.fingerprint) ||
      typeof draft.description !== "string"
    )
      return;
    return { ...draft, resources: Array.isArray(draft.resources) ? draft.resources : [] };
  } catch {
    return;
  }
}
/** Bind the displayed version to the actual originating tool call, including after replay. */
export function matchesSkillProposal(
  details: DraftDetails,
  proposal: SkillDraft,
  session: string,
  call: string,
) {
  if (
    details.draft.info.name !== proposal.name ||
    details.draft.info.fingerprint !== proposal.fingerprint
  )
    return false;
  try {
    const source = JSON.parse(details.draft.files["references/learning-source.json"] ?? "null");
    return source?.created_by_session === session && source?.created_by_tool_call === call;
  } catch {
    return false;
  }
}
