export const sessionCommands = [
  { name: "compact", description: "Compact conversation context" },
  { name: "review", description: "Review uncommitted changes in this workspace" },
  { name: "memory", description: "Extract memories from this conversation" },
] as const;

export type CommandName = (typeof sessionCommands)[number]["name"];
export type SessionCommand =
  | { type: "compact" }
  | { type: "memory" }
  | { type: "review"; target: { type: "uncommitted" } };

export function commandDraft(text: string) {
  // The + menu stores plugin selections as reference tokens in the draft.
  const match =
    /^((?:(?:@[a-z0-9]+(?:-[a-z0-9]+)*|@connector:[A-Za-z0-9_]{1,24})[ \t]+)*)(\/[a-z]*)$/i.exec(
      text,
    );
  return match && { prefix: match[1], token: match[2] };
}

export function opensCommandMenu(previous: string, next: string, inputType?: string) {
  const draft = commandDraft(next);
  return inputType === "insertText" && draft?.token === "/" && draft.prefix === previous;
}

export function matchingCommands(text: string) {
  const draft = commandDraft(text);
  if (!draft) return [];
  const search = draft.token.slice(1).toLowerCase();
  return sessionCommands.filter(({ name }) => name.startsWith(search));
}

export function commandRequest(name: CommandName): SessionCommand {
  return name === "review" ? { type: name, target: { type: "uncommitted" } } : { type: name };
}
