CREATE TABLE scheduler_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE scheduled_tasks (
 id TEXT PRIMARY KEY, revision INTEGER NOT NULL, config_json TEXT NOT NULL, creation_json TEXT NOT NULL,
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), deleted INTEGER NOT NULL DEFAULT 0,
 next_run_at INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE INDEX due_tasks ON scheduled_tasks(enabled,deleted,next_run_at);
CREATE TABLE scheduled_runs (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES scheduled_tasks(id),
 trigger_key TEXT NOT NULL, status TEXT NOT NULL, session_id TEXT NOT NULL UNIQUE,
 created_at INTEGER NOT NULL, run_json TEXT NOT NULL, UNIQUE(task_id,trigger_key)
);
CREATE INDEX task_runs ON scheduled_runs(task_id,created_at DESC,id DESC);
CREATE UNIQUE INDEX one_active_task_run ON scheduled_runs(task_id)
 WHERE status IN ('queued','starting','running','waiting_for_approval','waiting_for_input');
CREATE TABLE scheduler_leases (name TEXT PRIMARY KEY, owner TEXT NOT NULL, epoch INTEGER NOT NULL, expires_at INTEGER NOT NULL);
PRAGMA user_version = 1;
