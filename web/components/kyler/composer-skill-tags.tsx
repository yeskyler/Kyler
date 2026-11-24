"use client";

import { useEffect, useRef } from "react";
import { useAui, useAuiState } from "@assistant-ui/react";
import { BookOpenIcon, XIcon } from "lucide-react";
import { api } from "@/lib/kyler/client";
import {
  composerSkills,
  completedSkillNames,
  selectComposerSkill,
  setComposerSkills,
  type ComposerSkill,
} from "@/lib/kyler/composer-skills";
import { removeSkillReference } from "@/lib/kyler/plugin-references";
import type { Skill } from "@/lib/kyler/types";

export function ComposerSkillTags() {
  const aui = useAui();
  const text = useAuiState((state) => state.composer.text);
  const value = useAuiState((state) => state.composer.runConfig.custom?.kylerSkills);
  const running = useAuiState((state) => state.thread.isRunning);
  const selected = composerSkills(value);
  const sent = useRef<ComposerSkill[]>([]);
  const checked = useRef(new Set<string>());
  useEffect(() => {
    const reset = () => checked.current.clear();
    window.addEventListener("kyler-skills-changed", reset);
    return () => window.removeEventListener("kyler-skills-changed", reset);
  }, []);
  useEffect(() => {
    const clear = () => {
      const composer = aui.composer();
      const current = composerSkills(composer.getState().runConfig.custom?.kylerSkills);
      if (current.length) {
        sent.current = current;
        setComposerSkills(composer, []);
      }
    };
    const textChange = aui.on("composer.textChange", (event) => {
      if (event.messageId) return;
      const composer = aui.composer();
      if (event.origin === "send") clear();
      else if (event.origin === "return" && sent.current.length) {
        const current = composerSkills(composer.getState().runConfig.custom?.kylerSkills);
        setComposerSkills(
          composer,
          [
            ...current,
            ...sent.current.filter((item) => !current.some((old) => old.id === item.id)),
          ].slice(0, 16),
        );
      } else if (event.origin === "reset") sent.current = [];
    });
    const send = aui.on("composer.send", (event) => {
      if (!event.messageId && event.attachments > 0 && !aui.composer().getState().text) clear();
    });
    return () => {
      textChange();
      send();
    };
  }, [aui]);
  useEffect(() => {
    const names = completedSkillNames(text).filter((name) => !checked.current.has(name));
    const all = completedSkillNames(text);
    for (const name of checked.current) {
      if (!all.includes(name)) checked.current.delete(name);
    }
    if (!names.length) return;
    const controller = new AbortController();
    api<Skill[]>("plugins/skills", { signal: controller.signal })
      .then((skills) => {
        if (controller.signal.aborted || aui.composer().getState().text !== text) return;
        for (const name of names) checked.current.add(name);
        for (const skill of skills) {
          if (!names.includes(skill.name) || skill.status !== "enabled") continue;
          try {
            selectComposerSkill(aui.composer(), skill);
          } catch {
            break;
          }
        }
      })
      .catch(() => {});
    return () => controller.abort();
  }, [aui, text]);
  if (!selected.length) return null;
  return (
    <div className="flex flex-wrap gap-2 px-2.5 pt-2" aria-label="Selected skills">
      {selected.map((skill) => (
        <span
          key={skill.id}
          title={skill.label}
          className="inline-flex max-w-full items-center gap-1.5 rounded-lg border border-border bg-muted/70 py-1 pl-2.5 pr-1 text-xs text-foreground"
        >
          <BookOpenIcon className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="max-w-48 truncate">{skill.label}</span>
          <button
            type="button"
            disabled={running}
            aria-label={`Remove ${skill.label}`}
            className="rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
            onClick={() => {
              const composer = aui.composer();
              setComposerSkills(
                composer,
                composerSkills(composer.getState().runConfig.custom?.kylerSkills).filter(
                  (item) => item.id !== skill.id,
                ),
              );
              composer.setText(removeSkillReference(composer.getState().text, skill.name));
            }}
          >
            <XIcon className="size-3" />
          </button>
        </span>
      ))}
    </div>
  );
}
