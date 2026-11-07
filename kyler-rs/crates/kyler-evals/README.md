# Kyler evaluations

`kyler-evals` runs GAIA tasks through the real Kyler runtime and measures startup
in fresh subprocesses. It is separate from the interactive CLI and never runs
during installation. Dataset files and local reports are ignored by Git.

See the [evaluation overview](../../../docs/evaluations.md) for repository-level
setup and the [development guide](../../../docs/development.md) for shared checks.

Run the Cargo commands below from `kyler-rs/` (`cd kyler-rs` from the repository root).

## GAIA data

Place the downloaded dataset under `datasets/GAIA/2023/`, containing
`validation/metadata.parquet` and `test/metadata.parquet` and their attachments.
Legacy `metadata.jsonl` is also supported and takes precedence when present.
The native Rust reader needs no Python packages. A cloned Git repository may
contain only Git LFS pointer files: complete the authorized download separately.
The check command detects pointers and missing attachments without printing tasks
or reference answers. Do not redistribute the gated dataset or private reports.

```sh
cargo run -p kyler-evals -- gaia check
cargo run -p kyler-evals -- gaia run --limit 5
cargo run -p kyler-evals -- gaia run --level 1
cargo run -p kyler-evals -- gaia run
cargo run -p kyler-evals -- gaia run --split test
```

The default model configuration is `~/.kyler/config.toml`; `--app-config PATH`
selects another configuration. Model credentials are not copied into manifests.
`--eval-config crates/kyler-evals/config/gaia.toml` selects evaluation settings.
Paths in that file are relative to the file. The shipped file points to the same
dataset/results locations as the defaults. No third-party keys are requested or
downloaded by this program.

Each task gets an isolated workspace, attachment storage and Session/Rollout
store. The reference answer and annotator metadata are never supplied to the
Agent. The local sandbox denies dataset and report access. Results use a fixed
workspace-write boundary and approval policy `never`; no escalation or human
answers are available. Normal work inside that boundary is allowed. User memory
is not loaded. Skill installation, skill modification and memory writes are
denied, even when user skill reading is enabled.

There is no built-in web search. For web tasks, explicitly configure trusted MCP
servers in the application configuration and set `[agent].enable_mcp = true` in
the evaluation configuration. Interactive connector-store definitions are not
loaded or hot-reloaded. External services retain their own permissions; do not
connect services exposing benchmark labels or unrestricted host storage.
The data check does not establish network/provider/connector readiness.

User skills and subagents are opt-in. Enabled skills are copied to private
per-task storage, and source fingerprints are checked between tasks and on resume.
Default model/configuration snapshots remain fixed; tool definition fingerprints
and names observed on each model request are saved in `timings.json`.

Supported documents/images use the existing attachment API and also appear in the
task workspace. Unsupported media, archives and larger files (up to 256 MiB) use
the workspace path and are marked `workspace_only`; available tools must actually
handle them. A model lacking a required capability is not silently substituted.

Answers must end with one nonempty `FINAL ANSWER:` line. Missing/ambiguous markers
are format errors, not repaired by another model. The deterministic scorer follows
the numeric, ordered-list and text rules in the
[GAIA scorer](https://huggingface.co/spaces/gaia-benchmark/leaderboard/blob/9f133d7/scorer.py).
The scorer version is stored in each manifest. Token totals are provider-reported
and can undercount if a provider omits usage. Request counts cover runtime request
attempts; opaque SDK transport retries are not separately counted. Test predictions are unscored even
if metadata happens to contain an answer. Failed/timed-out validation tasks count
as incorrect; infrastructure errors remain visible in the denominator.

## Results and resume

Runs are stored in `results/<run-id>/`:

```text
manifest.json
summary.json
results.jsonl
tasks/<task-id>/attempt-<n>/
  result.json
  response.txt
  timings.json
  sessions/<session-id>/rollout.jsonl
  workspace/
```

The workspace is a bounded regular-file snapshot taken after cleanup; symlinks
are not followed. It is diagnostic evidence, not a resumable live environment.
`result.json` files are authoritative; `results.jsonl` and `summary.json` are
rebuildable indexes. An interrupted attempt is retained and receives a new attempt
directory on resume. Finished attempts are skipped unless `--retry-failed` is set.
Retries never overwrite previous evidence or select the best answer: the summary
uses the latest attempt. Changed executable/configuration/dataset/attachment
fingerprints require a new run. Ctrl+C cancels active work and preserves completed
results. Per-task timeout excludes bounded cancellation/cleanup grace periods.

```sh
cargo run -p kyler-evals -- gaia resume RUN_ID
cargo run -p kyler-evals -- gaia resume RUN_ID --retry-failed
cargo run -p kyler-evals -- report RUN_ID
cargo run -p kyler-evals -- compare BASELINE_ID CANDIDATE_ID
```

Run paths can be used instead of IDs. A partial/level-filtered score is not a full
GAIA score. Inspect the `complete`, `expected`, `scored` and failure counts.
No leaderboard submission, dataset download or model-based judging is performed.

## Startup performance

Build both binaries in release mode before interpreting measurements:

```sh
cargo build --release -p kyler-cli -p kyler-evals
target/release/kyler-evals startup run --scenario process-cold --profile minimal --repeat 30
target/release/kyler-evals startup run --scenario session-cold --mode mock --repeat 30
target/release/kyler-evals startup run --scenario session-warm --mode mock --repeat 30
target/release/kyler-evals startup run --scenario session-restore --restore-turns 100
target/release/kyler-evals startup run --scenario session-restore --restore-turns 100 --without-snapshot
```

`process-cold` launches the actual sibling `kyler serve` binary, or the executable
selected by `--kyler-bin`. The harness records process creation, entry to main,
Tokio runtime creation, config/logging, sandbox factory, storage, Agent and server
state initialization. Service readiness requires a successful HTTP `/healthz`
request, not just a listening port. The service binds a fresh loopback port and is
stopped with SIGTERM, with bounded force cleanup on failure. No installed service
is stopped, reused or restarted. Only Unix platforms currently support this path.

Every sample uses new private configuration/storage and a fresh process. Secrets
in temporary configuration are mode 0600 inside a temporary directory and are
removed after the sample; they are not put in reports. User filesystem grants,
server port overrides and daily session storage are not inherited. The default
profile is Minimal. There is no automatic system-cache purge or claim of a fully
cold OS/file cache. Actual binary build profiles and fingerprints are recorded;
using an installed/stale executable does not pick up source changes.

Session scenarios use a fresh evaluation subprocess and the real Agent runtime:

| Scenario | Measured work |
| --- | --- |
| `session-cold` | First turn without preconnecting MCP or scanning Skills |
| `session-warm` | One unmeasured prime turn, then another turn using the same Agent/Session |
| `session-restore` | Load a completed synthetic history from a snapshot or Rollout, then execute a turn |
| `runtime` | Legacy embedding initialization with capability prewarming |
| `server` | Legacy embedded server initialization, capability prewarming and HTTP health |
| `first-turn` | Legacy capability-prewarmed first turn |

Warm sessions reuse Agent/Session state; network connection reuse depends on the
runtime implementation and is not forced by the harness. Synthetic history is
created without model calls. Fixture generation time is not part of `restore_ms`,
but is visible in the overall process timeline. No user conversations are loaded.

### Profiles

```sh
target/release/kyler-evals startup run --scenario session-cold --profile default
target/release/kyler-evals startup run --scenario session-cold --profile skills --skill-count 32 --skill-instruction-bytes 4096
target/release/kyler-evals startup run --scenario session-cold --profile mcp --mcp-count 4 --mcp-delay-ms 100
```

- `minimal`: no user Skills, MCP or subagents; retains the normal core/server path.
- `default`: reads the selected application configuration, snapshots user Skills
  and retains explicitly configured MCP/subagents. Interactive connector-store
  definitions and user memory are not imported. Skills cannot modify their sources.
- `skills`: deterministic synthetic skill packages, prepared before timing begins.
- `mcp`: local JSON-RPC MCP fixtures with controlled initialization delay, prepared
  before timing begins; connections use the real MCP client/discovery path.

Use `--required-mcp ID` to declare a necessary connector in the Default profile.
A missing/failed required connector fails the sample. Other disconnected
connectors are reported as degraded capability samples, never silently called
fully ready. Synthetic MCP fixtures are always required. Process-cold does not
open a session, so its report explicitly leaves capability readiness unassessed;
choose a session scenario to measure deferred MCP/Skills costs.

### Model modes and clocks

`--mode mock` is the default and uses a local SSE model, with no paid model calls.
`--mode live` uses the configured provider and may incur normal model/tool usage.
Mock mode replaces only the model; Default-profile connectors can still contact
configured services. Use Minimal, Skills or MCP for fully local comparisons.

`--settings crates/kyler-evals/config/startup.toml` supplies defaults; CLI flags
win. Paths to binaries in the settings file are relative to that file.
`--timeout-seconds` bounds each sample including cleanup. Warmup, failed and capability-degraded samples are excluded from the main P50/P95.
Failures, timeouts, cancellations and degradation are counted separately, with a
separate degraded response-latency distribution and an explicit latency sample count. A failed measured sample makes the command
exit unsuccessfully after saving evidence.

On macOS/Linux, parent and child timestamps use the same `CLOCK_MONOTONIC` origin.
The cross-process entry time therefore includes loader/process creation costs,
without subtracting wall clocks. Unsupported clocks fall back to event receipt
and include transport latency. Stages may overlap and must not be blindly summed.
The first model event can be non-text and is not labeled a first-token metric.

The summary distinguishes service ready, Agent ready, capabilities ready,
turn-to-request, request-to-first-event, request-to-first-text, turn-to-first-text,
startup-to-first-text, turn total and restore latency. CPU/peak RSS cover only the
measured process (sampled after cleanup), not live workers/MCP servers. Debug builds
are useful only for verifying functionality, not performance conclusions.

### Comparing executables

```sh
target/release/kyler-evals startup run --scenario process-cold --kyler-bin ./target/release/kyler --baseline-bin /path/to/baseline/kyler --repeat 30
```

This alternates candidate/baseline and baseline/candidate order on successive
rounds. Each side gets independent fresh storage and its own warmups. Reports
include separate distributions, absolute differences and percentage changes.
Both binaries must contain the opt-in startup instrumentation; an older binary
without it is not a valid baseline and will fail the measurement. No model or
Agent behavior is changed by instrumentation. `KYLER_STARTUP_TRACE` is unset in
normal use, so startup trace messages are not emitted outside explicit probes.

## Tests

```sh
cargo test -p kyler-evals
```

Tests use synthetic metadata/attachments and local mock models. Integration tests
need localhost sockets and native MXC sandbox support, just like the existing
core sandbox tests. They never run the actual GAIA dataset or paid providers.
