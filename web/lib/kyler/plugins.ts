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
