import type { Connector, Skill } from "./types.ts";

export const skillSources = { builtin: "Built-in", user: "Custom", project: "Project" } as const;
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
      (!term || `${item.name} ${item.description}`.toLocaleLowerCase().includes(term)),
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
};
export function draftFilesReviewed(
  files: Record<string, string>,
  visited: readonly string[],
): boolean {
  const names = Object.keys(files);
  return names.length > 0 && names.every((name) => visited.includes(name));
}
