# Configuration

Kyler uses TOML. The repository template is `config/default.toml`; installation
writes the user's configuration to `~/.kyler/config.toml`. Kyler does not load
`.env` files automatically.

## File selection and precedence

The configuration file is selected in this order:

1. Explicit `--config PATH`.
2. `KYLER_CONFIG`.
3. `~/.kyler/config.toml`.

An explicit file must exist during normal loading; `configure` can create one.
Without a user file, built-in defaults are available,
but starting a service still requires valid model configuration.
For fields with a corresponding override, normal loading applies built-in defaults,
then TOML, then process environment, then explicit supported CLI options.

Managed service startup deliberately reuses complete model settings saved by
`kyler configure`, so stale shell model values do not undo installation answers.
Use `kyler configure --web` and restart to change installed settings. Foreground
`kyler serve` supports explicit `--model`, `--base-url`, and `--api-key` overrides;
prefer hidden setup or environment variables over keys in shell arguments.

Paths normally resolve beside the selected configuration. For the compatibility
case of a file inside a directory named `config`, application-relative paths
resolve from that directory's parent. `workspace.default_directory` and skill
paths support configured home-directory expansion. Sandbox protected paths are
workspace-relative; extra access roots follow configuration path resolution.

## Main settings

| Section | Purpose | Template/setup behavior |
| --- | --- | --- |
| `llm` | Model, API base URL, credentials, capabilities | Setup proposes `deepseek-ai/DeepSeek-V4-Flash` |
| `models` | Named provider/model profiles | When configured, replaces the legacy single `llm` profile |
| `agent` | Step limit and tool output budget | 50 steps; 8192-byte tool model-view budget |
| `context` | Window, compaction, output reservation | 128000 tokens; window ratio 0.6; trigger ratio 0.8 |
| `workspace` | Default task directory | `workspace`, normally `~/.kyler/workspace` |
| `session` | Persistent conversation storage | Template disabled; installer enables it |
| `subagent` | Delegation and child limits | Disabled by default; up to 32 concurrent and 64 total tasks per parent execution |
| `sandbox` | Filesystem, process, network policy | Workspace write; outbound networking enabled |
| `approval` | Tool decision policy and reviewer | `on-request`, user reviewer |
| `server` | Rust API and run limits | `127.0.0.1:30001` |
| `client` | Backend connection and stream settings | `http://127.0.0.1:30001` |
| `web` | Bundled Next.js service | Complete installer enables it at `127.0.0.1:30002` |
| `skills` | Skill discovery and limits | Built-ins plus configured user/project directories |
| `mcp` | Connector definitions and credential references | No external service is required by default |
| `logging` | Filtering and output format | `info`, compact output |

Consult `config/default.toml` and the corresponding Rust configuration types for
all fields and validation. Do not copy a configuration containing personal keys
into the repository.

## Environment overrides

| Variable | Purpose |
| --- | --- |
| `KYLER_CONFIG` | Configuration file |
| `KYLER_LLM_MODEL` | Single-profile model ID |
| `KYLER_LLM_BASE_URL` | Single-profile API base URL |
| `KYLER_LLM_API_KEY` / `OPENAI_API_KEY` | Model key, with the Kyler variable taking precedence |
| `KYLER_SERVER_BIND` / `KYLER_SERVER_TOKEN` | Backend listen address and token |
| `KYLER_CLIENT_SERVER_URL` / `KYLER_CLIENT_TOKEN` | Backend client endpoint and token |
| `KYLER_WEB_BIND` / `KYLER_WEB_TOKEN` / `KYLER_WEB_ENABLED` | Web listen address, password, and enabled flag |
| `KYLER_SESSION_DIR` / `KYLER_SESSION_ENABLED` | Session storage |
| `KYLER_MAX_STEPS` / `KYLER_LOG_FILTER` | Execution limit and log filter |

Named model providers and MCP connectors can reference additional credential
environment variables. Boolean overrides use `true` or `false`.
Setup captures supported configuration values into the saved file; later managed
service operations use those saved addresses rather than installation-shell
address overrides. Inspect the saved file when troubleshooting a difference.

## Permissions and updates

The interface offers Read-only, Workspace access, and Full access, subject to the
host ceiling. Sandbox policy controls actual access; approval controls whether a
call requires permission before execution. An approval cannot grant access that
the host has not authorized.

Workspace networking is enabled by default. To disable it:

```toml
[sandbox]
mode = "workspace-write"
network_access = false
```

Read-only is offline. Host policy edits require a service restart; existing session
bindings can retain narrower policies. Model profile changes through the interface
apply to the next turn, not a request already in progress.

See [installation](installation.md), [architecture](architecture.md), and
[troubleshooting](troubleshooting.md) for related behavior.

## Token metrics

Configuration, APIs, and persisted usage use whole integer tokens. Web and TUI
token metrics use decimal `k`: `1000 tokens = 1k tokens`, retaining up to three
decimal places. For example, `8192` is `8.192k`, not `8k`.

Context capacity comes from configuration; it is not inferred from a model name.
The most recent request's input occupancy is separate from cumulative session
input/output, which includes repeated requests and delegated work. Provider input
counts take precedence when available. Local counts and category breakdowns remain
estimates; category weights are allocated to the input total shown in the overview.

Streaming Chat Completions usage is accounted as per-request cumulative snapshots,
so repeated snapshots are not counted twice. Historical stored usage is
not rewritten: aggregate records cannot reliably recover an earlier provider's
original usage sequence. Raw Rollout JSON downloads retain integer values.

Managed server sessions use private execution directories beneath
`session.directory/_execution/<session-id>/workspace/`. The selected workspace
is used for project grouping; its existing files are not implicitly readable or
imported. Inputs must be supplied explicitly. Extra filesystem roots from the
global sandbox configuration do not expand this per-session boundary. The server
limits each session to read-only or workspace-write containment, even when the
host configuration permits unrestricted/external execution elsewhere.
