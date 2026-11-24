import type { Skill } from "./types.ts";
import { removeSkillReference, skillSelected } from "./plugin-references.ts";

export type ComposerSkill = { id: string; fingerprint: string; name: string; label: string };
type Config = { custom?: Record<string, unknown> };
export type SkillComposer = {
  getState(): { text: string; runConfig: Config };
  setText(text: string): void;
  setRunConfig(config: Config): void;
};
export function composerSkills(value: unknown): ComposerSkill[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 16)
    throw new Error("At most 16 skills may be selected.");
  const result: ComposerSkill[] = [];
  for (const item of value) {
    if (
      !item ||
      typeof item.name !== "string" ||
      !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(item.name) ||
      item.name.length > 64 ||
      typeof item.id !== "string" ||
      !/^[a-f0-9]{64}$/i.test(item.id) ||
      typeof item.fingerprint !== "string" ||
      !/^[a-f0-9]{64}$/i.test(item.fingerprint) ||
      typeof item.label !== "string"
    )
      throw new Error("Invalid selected skill. Remove it and select it again.");
    const previous = result.find((old) => old.id === item.id);
    if (previous && previous.fingerprint !== item.fingerprint)
      throw new Error("Conflicting skill versions selected.");
    if (!previous)
      result.push({
        id: item.id,
        fingerprint: item.fingerprint,
        name: item.name,
        label: item.label,
      });
  }
  return result;
}
export function skillReferences(value: unknown) {
  return composerSkills(value).map(({ id, fingerprint }) => ({ id, fingerprint }));
}
export function setComposerSkills(composer: SkillComposer, skills: ComposerSkill[]) {
  const { runConfig } = composer.getState();
  composer.setRunConfig({
    ...runConfig,
    custom: { ...runConfig.custom, kylerSkills: composerSkills(skills) },
  });
}
export function selectComposerSkill(composer: SkillComposer, skill: Skill, checked = true) {
  const state = composer.getState();
  let selected = composerSkills(state.runConfig.custom?.kylerSkills);
  if (checked) {
    if (skill.status !== "enabled") throw new Error("This skill is not enabled.");
    if (!selected.some((item) => item.name === skill.name))
      selected = [
        ...selected,
        {
          id: skill.id,
          fingerprint: skill.fingerprint,
          name: skill.name,
          label: skill.display_name ?? skill.name,
        },
      ];
  } else selected = selected.filter((item) => item.name !== skill.name);
  setComposerSkills(composer, selected);
  if (skillSelected(state.text, skill.name))
    composer.setText(removeSkillReference(state.text, skill.name));
}
export function completedSkillNames(text: string) {
  return [
    ...new Set(
      [...text.matchAll(/(?:^|\s)[@$]([a-z0-9](?:[a-z0-9-]*[a-z0-9])?)(?=\s)/g)]
        .map((match) => match[1])
        .filter((name) => name.length <= 64),
    ),
  ];
}
