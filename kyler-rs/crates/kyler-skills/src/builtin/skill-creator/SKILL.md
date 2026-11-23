---
name: skill-creator
description: Create or update reusable Kyler skills when the user wants to turn a workflow, project convention, or reference into repeatable agent instructions.
---
# Skill Creator

1. Start from a concrete request that should trigger the skill. Identify what
   Kyler needs to know, which steps repeat, and what proves the result is ready.
   Clarify a missing decision only when it changes the workflow, resources, or
   destination. For an update, inspect the current package and its resources first.
2. Choose a lowercase name of at most 64 characters using letters, digits, and
   hyphens, with no leading or trailing hyphen. Match the package directory.
   Create a `SKILL.md` with YAML `name` and `description`. Put both the capability
   and when to use it in the description. Start with the [skill template](assets/skill-template.md)
   when creating a package from scratch. Keep the body focused on prerequisites,
   execution, decision points, validation, and recovery.
3. Use flexible criteria for work that varies, a parameterized pattern for a
   common procedure, and a tested script for steps where exact execution matters.
   Put lengthy reference material under `references/`, executable helpers under
   `scripts/`, and output templates under `assets/`. Link each supporting
   resource from `SKILL.md` and explain when it is needed. See
   [package design](references/package-design.md) for a worked example.
4. Check the trigger against a realistic request and read the skill as the next
   agent would. Verify requested executable helpers with representative inputs
   under normal tool permissions. Remove duplicated prose, unused resources,
   credentials, and machine-specific details.
5. Use `save_skill` for the complete UTF-8 package, including unchanged resources
   on updates. For binary resources, prepare the full directory in the workspace
   and use `install_skill`. Default to user scope and use project scope for
   workspace-specific requests. Replace only the inspected version using its
   `expected_fingerprint`; reload and inspect if it changed. Use a user override
   to customize a built-in.

If the task is to extract a workflow from reviewed Memory sources, make an
uninstalled draft using exact source revisions and follow the draft review flow.
Report the saved name, scope, and status. Project packages require review and
enabling for their exact fingerprint.
