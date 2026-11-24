export type PluginReference = { kind: "skill" | "connector"; name: string };

export function pluginReference(reference: PluginReference) {
  const valid =
    reference.kind === "skill"
      ? /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(reference.name) && reference.name.length <= 64
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

export function skillSelected(text: string, name: string) {
  if (!/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(name) || name.length > 64) return false;
  const token = pluginReference({ kind: "skill", name });
  return text.split(/\s+/).some((part) => part === token || part === `$${name}`);
}

export function removeSkillReference(text: string, name: string) {
  const token = pluginReference({ kind: "skill", name });
  return text
    .split(/(\s+)/)
    .filter((part) => part !== token && part !== `$${name}`)
    .join("");
}
