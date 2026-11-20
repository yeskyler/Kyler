# Reviewed learning

Kyler separates facts and experience from reusable workflows. Memory stores scoped
facts and candidate lessons; Skills store procedures. The first implementation
provides explicit saves, candidate proposals, local review and hidden skill drafts.
It does not schedule background model calls, rewrite prompts/core code, execute
draft scripts, or automatically publish inferred skills.

## Memory lifecycle

The `memory` tool supports:

- `search`: only active, non-sensitive, unexpired entries are eligible for context.
- `save`: explicitly user-requested facts/preferences; updates require the current
  `expected_revision`. It cannot activate an existing candidate or rejected entry.
- `propose`: inferred facts, preferences, lessons or procedures become candidates.
  Add `metadata.kind`, `conditions`, `evidence` and optional `expires_at` (Unix
  milliseconds). Flagged sensitive content is rejected; redact before persistence.
- `list`: inspect entries in the current bound scope, optionally filtered by status.
  Continue with `next_offset`; individual reviews still require exact revisions.
- `delete`: requires `expected_revision`.
- `draft_skill`: generate an uninstalled user skill from exact scoped memory sources.

Source session/execution/tool-call identity is filled by the host, not accepted
from model arguments. Evidence is a bounded claim/reference, not an assertion that
Kyler independently verified it. Candidates never replace reviewed entries; use a
new key to propose a conflicting conclusion. Review can activate, reject or
invalidate an exact entry revision. Expired/invalid/rejected entries are excluded
from retrieval. Search includes at most 16 whole records within a 16 KiB record
budget. The existing scope derivation is preserved, including private-workspace
isolation; this release does not introduce globally shared personal memory.

The tool returns the store path, scope and candidate revision for review:

```sh
kyler --json learning memory --store /absolute/path/to/_memory --scope "$SCOPE" list
kyler learning memory --store /absolute/path/to/_memory --scope "$SCOPE" \
  review rust-tests --revision 1 --status active
```

Use `--status rejected` for a candidate that should not be used, or `invalid` for
an obsolete fact. These are local control-plane operations and make no model or
HTTP calls. Web uses its session store's `_memory`; TUI and ACP use their separate
session stores. Obtain the actual path and scope from the tool output rather than
assuming a different session shares them.

Existing `facts.json` records deserialize as active facts. Files remain the
single source of truth, with OS locking, atomic replacement and bounded sizes.
Revision checks protect updates and reviews. No automatic semantic deduplication,
source-file watching or full revision-history rollback is implemented yet.

## Skill draft lifecycle

For an explicitly requested reusable workflow, invoke `memory` with:

```json
{
  "action": "draft_skill",
  "name": "rust-validation",
  "instructions": "---\nname: rust-validation\ndescription: Validate Rust changes\n---\nDescribe prerequisites, steps, checks and recovery here.",
  "sources": [{"key": "rust-tests", "revision": 2}]
}
```

Sources must exist in the tool's bound memory scope with exact revisions, and
cannot be expired, invalid, rejected or marked sensitive. The generated package
contains `SKILL.md` and `references/learning-source.json`, which records source
identities and content hashes without copying raw memory content. These are
provenance snapshots at creation time, not live synchronization links.

Drafts live in the configured user skill directory's hidden `.drafts/` folder.
Discovery, context directories and skill loading exclude them; they cannot shadow
an installed skill. Package path, size, symlink and fingerprint checks still apply.
Draft updates require the draft's current fingerprint. User scope is the only
draft destination in this first release.

Review all files before explicitly publishing:

```sh
kyler --config /absolute/path/config.toml learning drafts list
kyler --config /absolute/path/config.toml learning drafts show rust-validation
kyler --config /absolute/path/config.toml learning drafts publish rust-validation \
  --fingerprint "$REVIEWED_DRAFT_FINGERPRINT"
```

Replacing an installed user skill additionally requires
`--expected-installed "$CURRENT_INSTALLED_FINGERPRINT"`. Publishing copies only
bytes matching the reviewed draft fingerprint through the normal skill validation
and atomic publication path. Existing disabled/review state still applies. The
draft remains available as an audit artifact; publication never executes scripts.
Ordinary explicitly requested `save_skill`/`install_skill` behavior remains under
its existing approval policy; do not use those tools to bypass draft review.

## Web review

Open the existing **Memory** page in the session inspector. Its memory list defaults to
candidates; inspect content, conditions, evidence and host-recorded provenance
before activating, rejecting or invalidating an entry. Recorded evidence still
requires independent review. Sensitive or expired entries cannot be activated.

Skill proposals appear as review cards in the originating assistant message, outside
its collapsed execution details. They remain available when conversation history is
reloaded. Cards bind the package fingerprint to its originating session and tool
call; changed packages require review of the newer proposal. Every provenance
source must belong to the session's memory scope.

Open every package file and confirm review, then choose **Install skill**. **Request
changes** adds an editable request with exact source revisions to the composer;
send it to generate a new version for review. **Discard** records a decision for
that exact version without installing it. Installation and discard decisions are
persisted in the user skill directory's hidden `.draft-reviews/` folder.
Publication installs a user skill available across sessions, subject to existing
enable/disable settings. It does not execute package scripts. **Plugins → Skills**
only manages installed skills; there is no Drafts tab.

Review remains part of Memory and Skills; there is no separate Learning page.
The authenticated HTTP endpoints are:

- `GET /v1/sessions/{id}/memory/entries` (cursor, limit, optional status).
- `GET /v1/sessions/{id}/memory/entry?key=...`.
- `POST /v1/sessions/{id}/memory/review`.
- `GET /v1/sessions/{id}/skills/drafts` (cursor and limit).
- `GET /v1/sessions/{id}/skills/drafts/{name}`.
- `POST /v1/sessions/{id}/skills/drafts/{name}/publish`.
- `POST /v1/sessions/{id}/skills/drafts/{name}/discard`.

Scope is derived from the stored session, never selected by client input. Review
requires the inspected scope and exact revision; publication requires the exact
draft fingerprint and, when replacing a skill, its installed fingerprint.
Conflicts return HTTP 409 and require opening the current version for review.
Project sessions sharing a memory scope see the same entries; isolated workspaces
retain separate scopes. The CLI remains available for local administration.

A TUI candidate review panel, automated validation suites, semantic conflict
detection, automatic invalidation and skill rollback are follow-up work. There is no automatic upgrade
from repeated success to active memory or an enabled skill.
