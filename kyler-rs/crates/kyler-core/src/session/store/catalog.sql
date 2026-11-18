BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS sessions (
 scope TEXT NOT NULL, session_id TEXT NOT NULL, title TEXT, cwd TEXT, project TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
 revision INTEGER NOT NULL, signature TEXT NOT NULL, summary_json TEXT NOT NULL,
 last_message_preview TEXT, is_draft INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(scope,session_id)
);
CREATE INDEX IF NOT EXISTS session_history ON sessions(scope,is_draft,updated_at DESC,session_id);
CREATE TABLE IF NOT EXISTS session_runs (
 scope TEXT NOT NULL, run_id TEXT NOT NULL, session_id TEXT NOT NULL, execution_id TEXT,
 status TEXT NOT NULL, last_committed_event_seq INTEGER NOT NULL,
 started_at INTEGER NOT NULL, finished_at INTEGER, PRIMARY KEY(scope,run_id),
 FOREIGN KEY(scope,session_id) REFERENCES sessions(scope,session_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS session_activity ON session_runs(scope,session_id,started_at DESC);
CREATE TABLE IF NOT EXISTS ui_preferences (
 scope TEXT PRIMARY KEY, last_session_id TEXT, sidebar_collapsed INTEGER NOT NULL DEFAULT 0,
 revision INTEGER NOT NULL DEFAULT 0
);
PRAGMA user_version=3;
COMMIT;
