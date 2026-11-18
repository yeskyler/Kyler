# Session persistence and browser recovery

Web session navigation is stored on the backend. Changing browsers or clearing
browser storage does not remove conversations, the recent published session, or
the saved sidebar layout. Connect to the same backend and sign in with its token.
This is a single-user preference scope; there is no separate browser user account.

## Storage

With the default configuration:

```text
~/.kyler/
├── app.sqlite3             # Session/run indexes and shared Web preferences
├── scheduler.sqlite3       # Independent automation definitions and history
└── sessions/
    └── <session-id>/
        ├── rollout.jsonl   # Authoritative committed conversation history
        └── snapshot.json   # Acceleration snapshot
```

A conventional `sessions/` store places `app.sqlite3` in its parent directory.
A standalone store with another directory name keeps `app.sqlite3` inside the
store itself. Tables are scoped by the canonical session-store path, so independent
stores sharing a database do not share history or preferences. Moving a store to a
new path rebuilds its index under a new scope; its old scoped preferences are not
automatically reassigned.

SQLite is bundled. No system SQLite package is required. The database uses WAL,
foreign keys, versioned migration, and Unix file permissions `0600`. Its `-wal`
and `-shm` sidecars are normal. Back up with SQLite backup tooling, or stop services
before copying the file. Do not copy only the main database during active WAL writes.
The database contains private paths and conversation previews; protect it like
conversation history.

## Responsibilities and consistency

- `sessions` indexes ID, title, cwd, project/layout, timestamps, revision, message
  counts, a bounded ordinary-message preview, and draft visibility. Session-list
  pagination queries SQLite rather than replaying each conversation.
- `session_runs` indexes durable run-manifest status, execution identity, timestamps,
  and the manifest's event watermark. This watermark is not a shared browser
  acknowledgement and does not make transient token events durable.
- `ui_preferences` saves the recent published session, sidebar state, and revision.
  Updates patch only supplied fields. Independent fields do not overwrite each
  other; conflicting edits use the last backend-committed write.

Rollout is committed before its session index is updated. A failed index update
does not undo a durable conversation checkpoint. The next local catalog read
repairs a known dirty index. On startup, file signatures identify missing or
outdated rows; only changed histories are replayed. Legacy JSON sessions are indexed
and retain the existing on-load Rollout migration. Removing a stale snapshot does
not invalidate the index or the authoritative journal. Run manifests remain the
admission/restart authority; their SQLite index is reconciled at service startup.

Deleting a session goes through the backend, cleans its indexes and associated
resources, and clears a matching recent-session preference. Drafts remain hidden
until publication. A draft cannot become the shared recent session.

## Navigation and active runs

1. An explicit `/chat/<session-id>` URL takes priority.
2. The home page restores the backend's recent session, if one is saved.
3. With no recent session, the home page opens a new draft. **New chat** explicitly
   starts a draft and clears the shared recent-session selection.
4. Opening an existing conversation queries its active run from the backend, then
   loads history and reconnects to that run's SSE stream. It never resubmits the user
   prompt to recover an existing run.
5. Sidebar preferences refresh on window focus. Shared recent navigation does not
   force another open browser to leave its current conversation.

No localStorage marker is required for session selection or run discovery. Unsent
composer drafts and unrelated document-preview settings retain their existing
browser-local behavior; they are not merged across browsers.

SSE token events still use bounded memory. If the replay window expired, the client
explicitly reports the gap and reconnects at the current run position. Some earlier
live fragments may be unavailable until saved history is reloaded. A completed
run's full final output is fetched/returned even when transient replay is gone.
A network/replay error alone never cancels a backend task; explicit Stop still does.
Server restart recovers recorded history and terminal/interrupted status rather
than automatically rerunning tools or restoring OS processes.

## Directory format and database migration

Session environments use `cwd` as their execution directory. Optional `project`
metadata groups sessions, and `managed.root` records host-owned private directories.
The removed directory aliases are not accepted in saved environments or creation
requests. Export or explicitly migrate old-format histories and automation records
before upgrading; the service does not relocate or delete them automatically.
Deleting a session never deletes an unmanaged project directory. Artifact snapshots
retain their session/execution ownership independently of cwd.

Application database schema version 3 stores `cwd` and `project` and removes the
obsolete directory columns. Versions 1 and 2 migrate automatically and session
summaries rebuild from authoritative history; shared Web preferences remain intact.
Older binaries cannot open schema version 3. Before upgrading, stop services and
back up both databases and the session store if rollback is required. Do not
downgrade against the upgraded data.
