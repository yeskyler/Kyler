---
name: skill-installer
description: Install or update Kyler skills from a local package or a user-selected Git repository.
---
# Skill Installer

Install the specific skill requested by the user. Default to user scope, or project scope when explicitly requested. Do not install unrelated packages or execute installation hooks.

## Local package

Use `install_skill` with the absolute path to the directory containing `SKILL.md`. The tool validates and copies the complete package, rejects symlinks and escaping paths, and never executes its scripts. Inspect existing package instructions and resources before updating; supply the installed package's current `expected_fingerprint` to replace it. A name collision without that fingerprint fails without overwriting anything.

## Git repository

Use `run_command` under the existing sandbox and approval policy to fetch the user's chosen repository into a temporary location in the workspace. Pass arguments directly to Git; do not build a shell command from the URL. Use a shallow clone with `--` before the repository URL, disable recursive submodules, and do not run repository scripts. If Git is unavailable or the fetch fails, report the cause and let the user supply a local package; do not invent another download source.

Locate the requested skill subdirectory. Do not import `.git` or the entire repository when it contains multiple skills. Inspect the package before calling `install_skill`. Never embed credentials in repository URLs or saved skills; use existing Git credential handling for private repositories.

After installation, report the source, destination, scope, fingerprint and `@skill-name` invocation. Project skills require review and enabling in `/skills`. Installed skills refresh on the next execution step; existing in-flight skill loads remain fingerprint-checked. Updating a built-in creates a user override and leaves bundled resources intact.
