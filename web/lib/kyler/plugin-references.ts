export type PluginReference = { kind: "skill" | "connector"; name: string };

export function pluginReference(reference: PluginReference) {
  const valid =
    reference.kind === "skill"
      ? /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(reference.name) && reference.name.length <= 64
      : /^[A-Za-z0-9_]{1,24}$/.test(reference.name);
  if (!valid) throw new Error("Invalid plugin reference.");
  return reference.kind === "skill" ? `@${reference.name}` : `@connector:${reference.name}`;
}

export function appendPluginReference(text: string, reference: PluginReference) {
  const token = pluginReference(reference);
  const tokens = text.split(/\s+/);
  if (
    tokens.includes(token) ||
    (reference.kind === "skill" && tokens.includes(`$${reference.name}`))
  )
    return text;
  return `${text}${text && !/\s$/.test(text) ? " " : ""}${token} `;
}
