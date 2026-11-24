import { appendPluginReference } from "./plugin-references.ts";
import type { Skill } from "./types.ts";

export const sessionCommands = [
  { name: "compact", description: "Compact conversation context" },
  { name: "memory", description: "Extract memories from this conversation" },
] as const;
export type CommandName = (typeof sessionCommands)[number]["name"];
export type SessionCommand =
  | { type: "compact" }
  | { type: "memory" }
  | { type: "review"; target: { type: "uncommitted" } };
export const commandGroups = ["System skills", "User skills", "Conversation"] as const;
export type SlashCommand =
  | {
      kind: "skill";
      name: string;
      description: string;
      group: "System skills" | "User skills";
      skill: Skill;
    }
  | { kind: "conversation"; name: CommandName; description: string; group: "Conversation" };
const reserved = new Set([
  "compact",
  "memory",
  "review",
  "resume",
  "model",
  "permissions",
  "mcp",
  "skills",
  "help",
]);
export function skillCommandName(name: string) {
  return name === "review-agent" ? "review" : reserved.has(name) ? `skill:${name}` : name;
}
export function commandDraft(text: string) {
  const match =
    /^((?:(?:[@$][a-z0-9](?:[a-z0-9-]*[a-z0-9])?|@connector:[A-Za-z0-9_]{1,24})[ \t]+)*)(\/[a-z0-9:-]*)$/i.exec(
      text,
    );
  return match && { prefix: match[1], token: match[2] };
}
export function opensCommandMenu(previous: string, next: string, inputType?: string) {
  const draft = commandDraft(next);
  return inputType === "insertText" && draft?.token === "/" && draft.prefix === previous;
}
export function matchingCommands(text: string, skills: Skill[] = []): SlashCommand[] {
  const draft = commandDraft(text);
  if (!draft) return [];
  const search = draft.token.slice(1).toLowerCase();
  const skillCommands: SlashCommand[] = skills
    .filter((skill) => skill.status === "enabled")
    .map((skill) => ({
      kind: "skill",
      name: skillCommandName(skill.name),
      description: skill.display_name ?? skill.name,
      group: skill.source === "builtin" ? "System skills" : "User skills",
      skill,
    }));
  const commands: SlashCommand[] = [
    ...skillCommands,
    ...sessionCommands.map((command) => ({
      ...command,
      kind: "conversation" as const,
      group: "Conversation" as const,
    })),
  ];
  const exact = commands.filter((command) => command.name.toLowerCase() === search);
  const unavailable =
    skills.some((skill) => skillCommandName(skill.name) === search) || search === "review";
  const matches = exact.length
    ? exact
    : unavailable
      ? []
      : commands.filter(
          (command) =>
            command.name.toLowerCase().startsWith(search) ||
            (command.kind === "skill" &&
              (command.description.toLowerCase().startsWith(search) ||
                (search.length >= 3 && command.description.toLowerCase().includes(search)))),
        );
  return matches.sort(
    (a, b) =>
      commandGroups.indexOf(a.group) - commandGroups.indexOf(b.group) ||
      a.name.localeCompare(b.name),
  );
}
export function commandRequest(name: CommandName): SessionCommand {
  return { type: name };
}
export function skillCommandDraft(text: string, skillName: string) {
  const draft = commandDraft(text.trimEnd());
  return draft ? appendPluginReference(draft.prefix, { kind: "skill", name: skillName }) : text;
}
export function reviewSkillDraft(text: string) {
  return skillCommandDraft(text, "review-agent");
}
