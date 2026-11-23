---
name: skill-installer
description: Install or update a Kyler skill from a local package or a user-selected Git repository, or help inspect and manage installed skills.
---
# Skill Installer

1. Identify the requested package and source. For Git, identify the repository,
   ref when specified, and exact package subdirectory. If the user only asks
   what is available, show skills from the available catalog and distinguish
   installed, disabled, shadowed, and review-required packages where those
   statuses are available. Use an external registry only when explicitly
   available and selected.
2. Inspect the local directory containing `SKILL.md`, including frontmatter and
   referenced resources. For Git, follow [the Git source workflow](references/git-sources.md)
   to acquire and inspect the requested package before import.
3. Use `install_skill` with the inspected directory and intended scope. Default
   to user scope and use project scope for a workspace-specific installation.
   The tool copies the package and validates its paths and resources.
4. If a package with the same name exists, inspect its source and current
   fingerprint. Replace it only when the requested operation authorizes an
   update, and pass the current `expected_fingerprint`. If it changed, inspect
   the new version before retrying. Customize built-ins through user overrides.
5. Report the installed name, scope, and returned status. Project packages need
   review and approval for the exact fingerprint before use. Use the available
   Skills UI or supported CLI controls for enablement and removal.

Fetching, inspecting, and installing a package never runs its scripts or grants
extra tool permissions. Use ordinary tool permissions for any separate execution
the user requested.
