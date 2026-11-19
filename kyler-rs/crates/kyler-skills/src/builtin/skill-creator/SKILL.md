---
name: skill-creator
description: Create or update a reusable Kyler skill from a user workflow, project convention, or reference material.
---
# Skill Creator

Create focused, reusable instructions that change how Kyler handles the requested task. Preserve the user's scope and choices. Prefer a concise skill over generic advice or duplicated documentation.

## Authoring

- Choose a lowercase name using letters, digits, and hyphens, at most 64 characters. Use the same name for the directory and YAML `name`.
- Write `SKILL.md` with YAML frontmatter containing `name` and a concise `description` explaining when the skill applies. Put instructions after the closing `---`.
- Keep names and metadata in English. Follow the user's language preference for instructions.
- Put substantial reference material in `references/`, reusable executable helpers in `scripts/`, and output templates in `assets/`. Create only resources the workflow needs; link them from the instructions.
- Include concrete decision criteria and completion checks. Do not claim additional permissions, request secrets in skill files, or override system instructions.

## Saving

Use `save_skill` with the entire package as UTF-8 `files`, including `SKILL.md`. Default to `scope: user`; use `project` when the user asks for a workspace-specific skill. The configured user directory is normally `~/.kyler/skills`, and the project directory is normally `.kyler/skills` in the session workspace.

For an update, read the current skill and its resources first, preserve unrelated content, and supply its current `expected_fingerprint`. Read user or project packages with normal file tools when needed; `load_skill` and `skills_resource` provide enabled skill content. To customize a built-in, save a user override without modifying bundled resources. Do not overwrite a conflicting local skill without inspecting it.

For binary assets, assemble the package in the workspace and use `install_skill` to import it; `save_skill` accepts UTF-8 files only.

Saving validates names, frontmatter, resource paths and size limits. Fix validation errors before retrying. Report the saved path, scope, and invocation `@skill-name`. Project skills require review and enabling in `/skills`; never claim they are enabled before confirmation. Skills refresh automatically on the next execution step. Scripts run only through ordinary tools under the existing sandbox and approval policy.
