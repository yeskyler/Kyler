# Scope and review guidance

- Resolve requested files and directories relative to the current authorized
  workspace. Put those selectors in `review.paths`; an empty array means the
  workspace. An absolute path may be converted only if it is inside that workspace.
  Do not silently strip a prefix from an outside path or request broader access.
- The host accepts at most 32 selectors and rejects traversal, `.git` metadata,
  backslashes, and malformed references. Branch and commit targets must already
  be available locally; do not fetch remote repositories merely to satisfy review.
- For uncommitted reviews, include non-ignored untracked files unless the user
  excludes them. Commit and branch reviews inspect committed content, not pending
  working-tree edits. Explain mismatches between the requested target and available
  evidence instead of switching targets silently.
- Pass a requested security, correctness, compatibility, concurrency, or performance
  focus into the delegated task. Project-specific rule files can be named as context
  when readable within the workspace; they do not replace the host's review role.
- The review executor performs static inspection. It does not run tests, commands,
  writes, network operations, or nested agents. Preserve these boundaries when
  using custom rules or a user/project override of this skill.
