# Development

Run commands from the repository root unless a different directory is specified. The
Rust workspace is inside `kyler-rs/`; the repository root has no Cargo manifest.

## Requirements

- The current stable Rust toolchain from [rustup](https://rust-lang.org/tools/install/),
  including Cargo, rustfmt, and support for Rust 2024.
- [Node.js 24](https://nodejs.org/en/download) and npm for Web development; release
  packaging uses the same native Node.js major version.
- Python 3.9+ for packaging and installer tests.
- macOS or Linux for native packaging and service execution. Native sandbox
  validation must run on each supported target platform.

On macOS, install command-line build tools with `xcode-select --install`. Ubuntu and
Debian builds require `build-essential`, `pkg-config`, and `libssl-dev`; native Linux
sandbox tests also need Bubblewrap and isolated-network dependencies listed in
[installation](installation.md#host-compatibility-and-sandbox-dependencies), plus a host
that permits user namespaces.
Additional native libraries are determined by the build dependencies. Install Python
through your operating system or the official Python distribution. Verify the
development tools before building:

```sh
rustc --version
cargo --version
node --version
npm --version
python3 --version
```

Prebuilt release installations do not require these development tools; see
[installation](installation.md).

## Rust

```sh
cargo check --manifest-path kyler-rs/Cargo.toml --workspace
cargo build --manifest-path kyler-rs/Cargo.toml -p kyler-cli
cargo fmt --manifest-path kyler-rs/Cargo.toml --all -- --check
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-config
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-cli --test service
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-cli --test doctor
```

Run the affected crate tests for scoped changes. Changes spanning multiple crates
require the corresponding integration suites or the complete workspace suite:

```sh
cargo test --manifest-path kyler-rs/Cargo.toml --workspace
```

Integration tests start local HTTP servers, subprocesses, PTYs, and native sandboxes.
Restricted execution environments may need permission to run those checks. Use
`--offline` only when the needed dependencies are already cached.

## Web

Start the Web development server in a separate terminal after configuring and starting
the backend:

```sh
cd web
npm ci
npm run dev -- --hostname 127.0.0.1 --port 30002
```

From `web/`, validate frontend work with:

```sh
npm run lint
npm test
npm run build
```

The frontend reads the backend connection from server-side configuration. The
development Web listener requires an available port. Installed and development services
must use separate ports when running concurrently. Integration tests must use isolated
workspaces and session stores.

## Configuration and local execution

For first-time backend setup from a checkout:

```sh
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- configure --web
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- serve
```

Both services use `~/.kyler/config.toml` by default. Skip `configure` when a valid
configuration already exists. Setup writes user configuration; use explicit `--config`
with a separate file to isolate development settings. See
[configuration](configuration.md) for path and override rules.

## Agent latency diagnostics

Set `KYLER_TRACE_LATENCY=1` when starting a development backend or TUI to enable
the `kyler_latency` log target (at `info` level). Restart the process to change
this setting. Diagnostics are disabled by default.

The logs correlate sessions and executions with turn admission, step preparation,
stream response headers, first model/reasoning/text events, and stream completion.
Request fingerprints, byte counts, token usage, and provider-reported cached and
reasoning tokens help compare equivalent requests without logging their contents.
Channel summaries report send counts, total and maximum send wait in microseconds,
and observations of a full queue. These summaries appear when the stream or
thread is released; a hosted thread normally closes after each turn. Send wait
includes scheduler time, and a full-queue observation is a point-in-time sample.
Cancelled sends may not finish their timing sample. Incomplete model requests
still produce a summary, including when a turn is cancelled.

Compare repeated requests with the same fingerprint, thinking setting, output
limit, cache usage, and output volume. The model read-wait measurement includes
SDK stream waiting; it does not separately measure DNS, TLS, or provider queueing.

## Installer and release checks

Build the CLI before running installer tests so integration cases are not skipped:

```sh
cargo build --manifest-path kyler-rs/Cargo.toml -p kyler-cli
python3 scripts/tests/test_install.py
python3 scripts/tests/test_release.py
python3 scripts/tests/test_package.py
sh -n scripts/install.sh scripts/package.sh scripts/release.sh
```

The installer tests use temporary homes and ports. Node.js is needed for the
complete-bundle fixture tests. Installer tests verify package activation, configuration
preservation, service startup, and rollback. Release validation also requires a
standalone Web startup check against the native bundle.

See [releasing](releasing.md) for producing a native package. Documentation validation
covers links, repository paths, and consistency with command-line help.
Documentation-only changes do not require an application build.

## Working files

Rust build output is stored in `kyler-rs/target/`, Next.js output in `web/.next/`, and
release output in `dist/`. Private GAIA datasets and local reports stay in the ignored
directories under `kyler-rs/crates/kyler-evals/`. Local data must be preserved during
directory moves and build cleanup.

## Automation scheduler

See [Automations](automations.md) for configuration, storage, and recovery behavior.
The independent `kyler-scheduler` crate uses bundled SQLite and a mockable execution
adapter; scheduling tests never call a provider. Run:

```sh
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-scheduler
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-server --test automations
```

The server integration test uses temporary storage and loopback-only fake model
responses, validating authorization, task creation/run idempotency, core admission,
and history persistence. It requires permission to bind local TCP sockets.

## Session catalog and cross-browser recovery

The session catalog uses bundled SQLite. See [Session persistence](session-persistence.md).
Validate migration, checkpoint reconciliation, deletion, shared preferences, and
reconnecting a fresh client without another model admission using:

```sh
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-core --lib session::store
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-server --test session_catalog
```

The server test uses temporary storage and a loopback fake model. It does not call
a paid provider or read an installed user's configuration.

## Tool concurrency

The core runtime executes adjacent explicitly parallel tools concurrently, with
`agent.max_parallel_calls = 8` by default (valid range: 1–64). File reads, searches,
directory listings, commands (`shell`, `run_command`, `exec_command`), attachment
reads and skill reads opt in. `get_task` is parallel; `wait_tasks` uses a separate
lane with the same limit so waiting cannot exhaust command slots. Process input,
patches, skill writes, session mutations, questions, task creation/cancellation,
MCP calls and undeclared extension tools remain exclusive barriers. Tools before
a barrier finish before it starts, and later tools wait for it to finish.

Each agent runtime has its own limits; these are not workspace-wide locks.
Background processes and child agents may outlive their initiating tool call.
The model must keep dependent commands and conflicting writes in separate batches;
the runtime does not infer command dependencies or filesystem conflicts.

Tool authors opt in through `Tool::concurrency()` and implement
`execute_parallel()` using `ParallelToolContext`, which provides execution services
without mutable session state. Approval and sandbox strategy are independent of
concurrency. Authorization remains ordered and completes for a parallel group
before its execution starts. Execution slots are acquired only after authorization.
Results are checkpointed and streamed as tools finish; model messages order them
by the original calls while retaining their call IDs. Cancellation drops pending
tool futures and the existing execution cleanup handles managed processes/tasks.

`agent.parallel_tool_calls` is an optional model-request hint, independent of
runtime concurrency. Leave it unset for provider compatibility, set it to `true`
for supporting endpoints, or `false` to ask the model for sequential tool use.
It does not override runtime barriers or permissions. Tool execution begins after
the model response completes, not while argument deltas are still arriving.
