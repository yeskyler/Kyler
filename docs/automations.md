# Automations

Open **Automations** in the sidebar to create scheduled agent tasks. The backend
must remain running; the browser can be closed. Saving or previewing a task does
not call a model. **Run now** and scheduled executions do call the selected model.

## Configuration

1. Enter a name, instructions, and an authorized workspace. **Project directory**
   runs in the selected project and can change it according to the selected
   permissions. **Isolated directory** creates a separate execution directory;
   it does not automatically copy the project contents.
2. Choose a schedule and an IANA time zone, such as `Asia/Shanghai`. Custom schedules
   use five-field Cron expressions, with minute-level precision. Preview shows the
   next five occurrences in the selected time zone.
3. Select a configured model profile, thinking level, permissions, execution timeout,
   and approval/input wait timeout. Full access requires explicit confirmation for
   each saved task revision and remains constrained by the host execution boundary.
4. Save paused, or save and enable. Run now also works for paused tasks.

Each run receives a separate conversation. Open run history to inspect results,
open the conversation, answer pending questions or approvals, or stop an active run.
Editing or pausing a task does not modify an already queued/running snapshot. Stop
that run separately when needed. Stop unfinished work before deleting a task.
Deletion hides the task but retains its history in SQLite.

## Execution and recovery

`kyler-scheduler` owns schedules, SQLite transactions, leases, and run history.
`kyler-server` adapts admitted runs to the existing core thread operations and
RunManager. It does not introduce another agent loop.

- At most two automation runs execute concurrently, further limited by the host's
  run capacity. Each task may have only one unfinished run.
- An overlapping scheduled occurrence is recorded as skipped. Offline occurrences
  are skipped on restart; there is no automatic catch-up or failure retry.
- Saved model profile, workspace, thinking, permissions, and limits form an immutable
  snapshot for each run. Removing a profile or revoking workspace authorization
  causes a subsequent run to fail validation.
- A durable run/session ID is committed before admission. Repeated manual request
  IDs return the existing run; uncertain admission is never blindly replayed.
- On restart, queued intent is retained, admitted runs are reconciled, and unknown
  outcomes interrupt the run and pause its task. Inspect any side effects before
  enabling or manually rerunning it.
- Execution time excludes known approval/input waits. Wait time has its own limit.
  Cancellation is requested at the limit; terminal status requires core confirmation.
  Host execution and approval limits also apply and cannot be expanded by the task.
- Spring daylight-saving gaps are skipped; autumn repeated local times run only at
  the first occurrence. The database stores UTC milliseconds.

## Storage and operations

For the default session directory, data lives in `~/.kyler/scheduler.sqlite3`, next
to `sessions/`. SQLite is bundled into the binary; no system SQLite installation
is required. The database uses WAL, foreign keys, versioned migration, and Unix
file permissions `0600`. Back up the database consistently using SQLite backup
tooling, or stop the backend before copying it. Do not copy only the main file
while WAL writes are active.

The database is bound to its session store. Do not point independent session stores
at the same scheduler database. A renewable lease and fenced writes prevent two
scheduler owners from dispatching the same durable intent. The scheduler starts and
stops with `kyler serve`; embedded `Server::with_agent` instances do not enable it.
The Web proxy forwards authenticated `/v1/automations` and
`/v1/automation-runs/{id}/cancel` routes. Task definitions contain prompts and paths,
so protect this database like conversation history; credentials stay in model
configuration and are not stored in task snapshots.

The initial limits are 256 visible tasks, a 32,000-byte prompt, and per-task execution
and wait limits between 1 and 86,400 seconds. Run history uses cursor pagination.
