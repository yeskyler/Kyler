---
name: review-agent
description: Review code changes in a delegated read-only agent when the user requests code review of uncommitted changes, a commit, or changes relative to a local branch. Supports workspace-relative file and directory scopes.
metadata:
  display-name: Review Agent
---
# Review Agent

Review the requested changes and return actionable findings without modifying code.
Use this skill for requested code review; selecting it does not authorize fixes.

## Target and paths

Use the user's target and scope. If no target is specified, review uncommitted
changes. Use `base_branch` for committed HEAD relative to a local branch's merge
base, or `commit` for a specific local commit against its first parent.

Read [scope and review guidance](references/review-guidelines.md) when translating
file paths, branch names, or review focus into the delegated request. Keep review
paths separate from this skill's source path and resource paths. Skill resources
are resolved relative to this package using `skills_resource`; do not write a
machine-specific absolute skill path into the request.

## Delegate and collect

Call `delegate_task` with `agent: "review"`, a self-contained `task`, and a `review`
object. Pass user-selected files/directories as `review.paths`. For example:

```json
{
  "agent": "review",
  "task": "Review the requested changes for actionable defects. Do not apply fixes.",
  "review": {
    "target": { "type": "uncommitted" },
    "paths": ["web/", "kyler-rs/crates/kyler-server/"],
    "include_untracked": true
  }
}
```

For a branch use `{"type":"base_branch","branch":"main"}`; for a commit use
`{"type":"commit","commit":"<local commit ID>"}`. Include the user's review
focus and applicable rules in `task` or `context`; never expand the requested scope.
If delegation is unavailable, explain that the isolated Review Agent could not run
rather than presenting an ordinary response as its validated report.

Use `wait_tasks` to collect the completed task before responding. The host captures
the Git evidence, restricts the child to review tools, validates the structured
report, and detects stale evidence. Neither skill text nor path selection grants
additional filesystem or execution permissions.

Read [report presentation](references/report-format.md) when presenting the result.
