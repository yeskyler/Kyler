# Configuration

Kyler uses TOML. The repository template is `config/default.toml`; `kyler configure` and
the installer create the user configuration at `~/.kyler/config.toml`. Kyler does not
load `.env` files automatically.

## File selection and precedence

The configuration file is selected in this order:

1. Explicit `--config PATH`.
2. `KYLER_CONFIG`.
3. `~/.kyler/config.toml`.

An explicit file must exist during normal loading; `configure` can create one. Built-in
defaults apply when the default file is absent. Service startup requires a valid model
configuration and enabled session persistence. For fields with a corresponding override,
normal loading applies built-in defaults, then TOML, then process environment, then
explicit supported CLI options.

Managed services use the complete model settings saved by `kyler configure`. Model
overrides in the invoking shell do not replace these saved settings. Use `kyler
configure --web` and restart to change installed settings. Foreground `kyler serve`
supports explicit `--model`, `--base-url`, and `--api-key` overrides; supply credentials
through hidden setup input or process environment variables; command-line arguments are
visible in shell history and process listings.

`kyler configure` asks for the context window after the model name. Enter keeps
the current value; `128k` means 128000 tokens. Legacy single-model setup saves
`[context].context_tokens`; named-profile setup saves each profile's `context_tokens`.
The value must match the selected provider's model limit and pass the runtime budget
validation. Changing the model name does not automatically detect or reset its window.
After saving, restart the backend or explicitly reload the model catalog; an admitted
turn retains its existing snapshot.

Paths normally resolve beside the selected configuration. When the selected file is
inside a directory named `config`, application-relative paths resolve from that
directory's parent. `workspace.default_directory` and skill paths support configured
home-directory expansion. Sandbox protected paths are workspace-relative; extra access
roots follow configuration path resolution.

## Main settings

| Section | Purpose | Template/setup behavior |
| --- | --- | --- |
| `llm` | Model, API base URL, credentials, capabilities | Setup default: `deepseek-ai/DeepSeek-V4-Flash` |
| `models` | Named provider/model profiles | When configured, replaces the legacy single `llm` profile |
| `agent` | Step limit, tool concurrency and output budget | 50 steps; 8 parallel calls; 8192-byte tool model-view budget |
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

The complete field reference is defined by `config/default.toml` and the Rust
configuration types. Configuration files containing credentials must remain outside
version control.

## Tool concurrency

`agent.max_parallel_calls` defaults to 8 and accepts 1–64. Independent reads and
commands can overlap; file edits and session mutations remain ordered barriers.
Task waits have a separate lane with the same limit. Limits apply per agent
runtime, not across sessions or child agents, and do not bound the lifetime of
background processes. Setting the limit to 1 serializes ordinary parallel tools;
task waits can still overlap them.

`agent.parallel_tool_calls` separately controls the optional model request hint.
Leave it unset to use provider defaults, or set it to `true`/`false` when the
endpoint supports it. This hint does not change tool permissions or runtime
barriers. See [Tool concurrency](development.md#tool-concurrency) for tool
classification and authoring details.

## Environment overrides

| Variable | Purpose |
| --- | --- |
| `KYLER_CONFIG` | Configuration file |
| `KYLER_LLM_MODEL` | Single-profile model ID |
| `KYLER_LLM_BASE_URL` | Single-profile API base URL |
| `KYLER_LLM_API_KEY` / `OPENAI_API_KEY` | Model key, with the Kyler variable taking precedence |
| `KYLER_SERVER_BIND` / `KYLER_SERVER_TOKEN` | Backend listen address and token |
| `KYLER_CLIENT_SERVER_URL` / `KYLER_CLIENT_TOKEN` | Backend client endpoint and token |
| `KYLER_WEB_BIND` / `KYLER_WEB_TOKEN` / `KYLER_WEB_ENABLED` | Web listen address, access token, and enabled flag |
| `KYLER_SESSION_DIR` / `KYLER_SESSION_ENABLED` | Session storage |
| `KYLER_MAX_STEPS` / `KYLER_LOG_FILTER` | Execution limit and log filter |

Named model providers and MCP connectors can reference additional credential environment
variables. Boolean overrides use `true` or `false`. Setup captures supported
configuration values into the saved file; later managed service operations use those
saved addresses rather than installation-shell address overrides. The saved
configuration is the reference for managed-service diagnostics.

## Permissions and updates

The interface offers Read-only, Workspace access, and Full access, subject to the host
ceiling. Sandbox policy controls actual access; approval controls whether a call
requires permission before execution. An approval cannot grant access that the host has
not authorized.

With the default `workspace-write` configuration, Full access is available without
setting `mode = "danger-full-access"`. Sessions still start with Workspace access.
Selecting Full access requires confirmation and an idle session with no running managed
processes. It switches local tool execution to unrestricted filesystem and network
access as the service OS user, disables per-call approvals, and keeps the current
working directory. OS and external container restrictions still apply. Existing MCP
connections retain their configured execution boundary.

Users select Full access in the session permissions interface. Read-only and
external-sandbox execution environments
retain their existing restrictions.

Workspace networking is enabled by default. To disable it:

```toml
[sandbox]
mode = "workspace-write"
network_access = false
```

Read-only is offline. Host policy edits require a service restart; existing session
bindings can retain narrower policies. Model profile changes through the interface apply
to the next turn, not a request already in progress.

See [installation](installation.md), [architecture](architecture.md), and
[troubleshooting](troubleshooting.md) for related behavior.

## Token metrics

Configuration, APIs, and persisted usage use whole integer tokens. Web and TUI token
metrics use decimal `k`: `1000 tokens = 1k tokens`, retaining up to three decimal
places. A count of `8192` tokens is displayed as `8.192k`.

Context capacity comes from configuration; it is not inferred from a model name. The
most recent request's input occupancy is separate from cumulative session input/output,
which includes repeated requests and delegated work. Provider input counts take
precedence when available. Local counts and category breakdowns remain estimates;
category weights are allocated to the input total shown in the overview.

Streaming Chat Completions usage is accounted as per-request cumulative snapshots, so
repeated snapshots are not counted twice. Persisted historical usage remains unchanged
because aggregate records do not preserve the original provider usage sequence. Raw
Rollout JSON downloads retain integer values.

The session composer footer shows the latest completed primary-agent model request's
average output rate in `tokens/s`, followed by Context, Memory, and Rollout controls.
The rate uses provider-reported completion tokens divided by model request time,
including connection and first-token latency and excluding tools. Requests without
usable token counts or timing show `— tokens/s`. The measurement is retained in the
session checkpoint and is separate from cumulative usage and subagent activity.

## Session inspection charts

Context, Memory, and Rollout use the same card layout and read-only charts.
Context distinguishes provider-reported input from local category estimates and marks
model/budget changes and compression events. Summary and fact contents remain readable
text, with complete details in drawers.

`GET /v1/sessions/{id}/analytics` summarizes the complete committed rollout, independently
of timeline pagination. Event updates are deduplicated by event identity, and tool
results are matched within their execution. The response includes the latest 300
request snapshots, latest 100 compression snapshots and execution summaries, up to
24 activity buckets, and the top 20 tools plus an aggregate of remaining tools.
Arguments, messages and tool outputs are excluded. Charts refresh when inspection
opens, a turn changes state, the window regains focus, or Refresh charts is selected;
the full rollout is not rescanned on the timeline's polling timer.

Memory activity charts cover the API's latest 30 explicit operations, including failures.
Tag counts measure facts per tag and can overlap. No retrieval hit-rate or memory quality
score is inferred. Timeline details remain paginated and are fetched on demand; raw
event downloads retain the existing redaction and preview limits.

## Session execution paths

Managed server sessions use private execution directories beneath
`session.directory/_execution/<session-id>/workspace/`. The selected workspace is used
for project grouping; its existing files are not implicitly readable or imported. Inputs
must be supplied explicitly. Extra filesystem roots from the global sandbox
configuration do not expand the default per-session boundary. Explicitly confirmed Full
access can remove this containment; the session
continues using the same private working directory.

## Model thinking mode

Agent requests automatically send `enable_thinking = true` for
`deepseek-ai/DeepSeek-V4-Flash` on `api.siliconflow.cn` or `api.siliconflow.com`.
Existing configurations without the setting use this default. Other model/provider
combinations omit the provider-specific parameter unless explicitly configured.

To disable it, set `enable_thinking = false` under `[llm]`. When named profiles are
configured, set it under the corresponding `[models.profiles.<name>]` instead.
An explicit `true` also supports compatible custom gateways. Model selection and
reload retain the setting within the execution's immutable model snapshot.

Streaming reasoning appears separately from the final answer. Current-turn tool
continuations include the associated `reasoning_content` and count it in the input
budget. Reasoning from prior turns is not resent. Thinking may increase response
latency and token usage.

The Web composer displays the current model and thinking level on the right. Its
Model and Thinking level submenus select the session model and Off, Low, High, or Max.
The saved session choice overrides the configuration default and applies at the next
turn without a service restart; active executions keep their captured setting.
Sessions default to High. Off sends `enable_thinking: false` without `reasoning_effort`.
Low, High, and Max enable thinking and send `reasoning_effort: "low"`, `"high"`, or
`"max"`, respectively. Saved Off and legacy boolean false preferences remain Off.
A bound model accepts explicit preferences without a local model-name allowlist.
Provider compatibility is validated by the provider; unsupported request parameters
remain errors rather than being silently ignored or mapped to identical requests.

Editing the configuration file still requires model reload or restart. Reload updates
model defaults, while an existing session override retains precedence. See the
[SiliconFlow request parameters](https://docs.siliconflow.cn/docs/api/chat-completions-post).

Web access uses `[web].access_token` in both local and network modes. Setup generates
it automatically; `kyler service token` displays it and `--reset` rotates it. The
browser exchanges this token for a signed session cookie. Managed Web services read
the token from the configuration so rotation invalidates old cookies without a restart.

## MCP tool retrieval

MCP metadata retrieval uses local field-weighted BM25, without embeddings or a
vector database. Each new task initially advertises only `search_tool_bm25` for
MCP discovery, regardless of catalog size. The model searches with up to four
queries and optional `server_ids`; matching tools that fit the activation budgets
are loaded with full schemas in the next model request. There is no separate load
call. Search results contain summaries and required parameter names, not full
schemas. English identifiers, camel-case components, Chinese character fragments
and a small bilingual action/resource dictionary are supported.

```toml
[mcp.search]
enabled = true
max_active_tools = 6
schema_token_budget = 4000
explicit_wait_ms = 3000
```

`max_active_tools` accepts 1–64; `schema_token_budget` accepts 128–65536 and is
counted with the selected model tokenizer, including approval schema metadata.
Oversized schemas are skipped rather than truncated. Increase the budget if a
required tool reports that it cannot be activated. Task waits and built-in tools
are outside this MCP-specific schema budget. Setting `enabled = false` restores
full advertisement of currently connected MCP tools.

Connecting servers no longer block unrelated model requests. Only explicit
`@connector:id` mentions wait, for at most `explicit_wait_ms` (0–10000). An explicit
mention does not enable a disconnected connector; existing configured connection
and sandbox rules still apply. `search_tool_bm25` reports connection states when some
servers are not ready. Newly activated tools become available on the next model
request. Configured but disconnected servers also keep the search entry visible;
search returns their status without connecting them. Active tools are retained
within the current task and recent calls take
priority when later selection changes. A new task starts with a fresh selection.
Catalog updates, disconnects and reconnects refresh availability and invalidate
stale schemas; remote side-effecting calls are never automatically replayed.

## Skills and directory budgets

Skills use progressive loading, not BM25 tool selection. The model sees metadata
for enabled packages and chooses relevant guidance by description. Explicit
`@skill-name` / `$skill-name` inputs are loaded by core before the first model
request. API clients may instead send an exact reviewed reference:

```json
{
  "input": "Use the selected skill for this task",
  "skills": [{"id": "<64-character skill ID>", "fingerprint": "<64-character package fingerprint>"}]
}
```

Obtain IDs and fingerprints from the authenticated skill directory; arbitrary
paths are not accepted. At most 16 explicit references are allowed. Conflicting
versions, disabled/shadowed packages and changed fingerprints fail before loading
instructions into the model. Resuming a task retains its explicit selections and
revalidates their versions and current review status.

```toml
[skills]
catalog_token_budget = 4096
full_scan_interval_ms = 30000
```

`catalog_token_budget` is measured with the selected model tokenizer, including
the directory header, metadata and omission notice. It does not truncate skill
instructions. Use `list_skills` when the context directory reports omitted skills;
follow `next_offset` and `expected_version` to avoid mixing catalog generations.
`load_skill` loads the full instructions; `skills_resource` reads references and
scripts as text without executing them.

`full_scan_interval_ms` accepts 0–300000. Metadata checks run between full content
scans, so ordinary edits invalidate the cache promptly. Zero forces a complete
rescan for every snapshot. Reads of instructions and resources always validate
content fingerprints regardless of this interval. Package writes clear directory
caches immediately; enable/disable review records are checked afresh. User and
built-in guidance is enabled by default unless disabled; project packages require
review of the exact fingerprint. Script execution uses ordinary tool permissions.
