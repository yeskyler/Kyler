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

`kyler configure` uses an inline keyboard wizard to select a provider, request its
model directory, and search or choose a model. It does not send inference requests. Discovery failures
allow cached IDs, retry, or manual input. Non-interactive setup uses the supplied
configuration/environment and never requests a remote directory. In the wizard,
Esc goes back, Ctrl+C cancels, and the final review can save, edit, or cancel the
draft. API keys remain hidden, including completed-question summaries.

New installations use `[context].auto_model_window = true`. The runtime uses
verified capacity metadata when known, with `context_tokens` as a fallback budget
(default 128000), not an assertion about an unknown model's capacity. Existing
explicit window settings remain unchanged. Named profiles use
`auto_context_window = true` for the same behavior, or an explicit `context_tokens`
budget. Runtime binds the effective window to each execution snapshot.

Ordinary setup omits the context question. Run `kyler configure --advanced` to enter
an explicit limit (`128k` = 128000), or `auto` to follow model metadata. The selected
profile is updated; other profiles retain their budgets. After saving, restart the
backend or reload its model catalog. An admitted turn retains its existing snapshot.

Paths normally resolve beside the selected configuration. When the selected file is
inside a directory named `config`, application-relative paths resolve from that
directory's parent. `workspace.default_directory` and skill paths support configured
home-directory expansion. Sandbox protected paths are workspace-relative; extra access
roots follow configuration path resolution. In `workspace-write` mode, `.git`
inherits workspace access. Opening a directory leaves repository creation to
an explicit Git operation. Use `sandbox.protected_paths` to make Git metadata
read-only for a deployment. Tool approval and network policy apply independently.

## Main settings

| Section | Purpose | Template/setup behavior |
| --- | --- | --- |
| `llm` | Model, API base URL, credentials, capabilities | Setup default: `deepseek-ai/DeepSeek-V4-Flash` |
| `models` | Named provider/model profiles | When configured, replaces the legacy single `llm` profile |
| `agent` | Step limit, tool concurrency and output budget | 50 steps; 8 parallel calls; 8192-byte tool model-view budget |
| `context` | Window, compaction, output reservation | 128000 tokens; window ratio 0.6; trigger ratio 0.8 |
| `workspace` | Default task directory | `workspace`, normally `~/.kyler/workspace` |
| `session` | Persistent conversation storage | Template disabled; installer enables it |
| `subagent` | Delegation and child limits | Enabled by default; up to 32 concurrent and 64 total tasks per parent execution |
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
| `KYLER_SERVER_BIND` / `KYLER_GATEWAY_TOKEN` | Backend listen address and token |
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

## Providers and model discovery

`kyler-provider` exposes one preset catalog to `kyler configure`. Agent execution
uses the same model and tool interface for every preset.

| Group | Provider presets |
| --- | --- |
| Direct | OpenAI, Anthropic Claude, Google Gemini, xAI, Mistral, Cohere, Perplexity, DeepSeek, Qwen (Alibaba Cloud Model Studio), Kimi, GLM, MiniMax, Volcano Engine Ark, Tencent Hunyuan, Baidu Qianfan, SiliconFlow |
| Hosted inference | OpenRouter, Groq, Together AI, Fireworks AI, Cerebras, Hugging Face Inference Providers |
| Cloud | Azure OpenAI / Microsoft Foundry, AWS Bedrock, Google Vertex AI |
| Local | Ollama, LM Studio, vLLM, llama.cpp |
| Custom | OpenAI-compatible endpoint |

Claude uses Anthropic Messages and Gemini uses GenerateContent. The cloud presets
use OpenAI-compatible inference. Configure the resource and deployment for Azure,
the regional endpoint and a Bedrock API key for AWS, or the Vertex project and
location endpoint with a Google Cloud access token. Vertex tokens must be
refreshed by the operator. Use a manually selected model or deployment name
when its endpoint lacks a model directory. Local presets accept an empty API key.

Use Off for Claude and Gemini while native reasoning controls are unavailable.
Gemini models can still apply their own default thinking behavior. Tool and image
availability can vary by model, region, and endpoint. The preset catalog describes
connection setup. It does not guarantee support for every model capability.
Existing `[llm]` and named profiles remain supported. `adapter` accepts `auto`
(the default), `openai`, `siliconflow`, or `generic`. Auto identifies official
endpoints by their exact hostname; specify an adapter for a compatible proxy.

```toml
[models.providers.openai]
adapter = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
discover_models = true
```

Providers can be configured without manual profiles. Discovery fetches `GET /models`
(SiliconFlow adds `sub_type=chat`) on startup and every five minutes. It runs in the
background, with at most four concurrent requests and a ten-second timeout per
provider. TUI's model reload also refreshes the directory.
No inference request is used to discover or probe capabilities. The first discovered
catalog chooses a default when none was configured; later refreshes do not silently
replace that default or an explicit selection. If a selected model disappears,
select another model. Before first discovery, a provider-only setup has no usable
model; a configured profile is available immediately.

The host atomically saves credential-free model IDs and validated capability declarations in
`session.directory/_model-catalog.json` when session storage is enabled. A cache
entry is scoped to the provider, endpoint, adapter and credential fingerprint.
Failures preserve previously discovered models and expose a safe per-provider
status through `/v1/models`. Credentials and provider response bodies are never
stored in this cache. Set `discover_models = false` to use manual profiles only.

Model lists may omit capability details. Reasoning capability precedence is explicit
per-model configuration, validated provider metadata, then adapter rules. Unknown
capabilities remain unknown; the installation model never defines other models'
levels. A model-list entry may provide `capabilities.reasoning` (or `reasoning`)
with explicit `levels` and an optional supported `default`. Boolean `false` means
unsupported; `true` without levels does not establish valid effort values. Invalid
or duplicate level declarations are ignored, and provider-controlled source text
is discarded. Metadata applies to configured profiles as well as discovered ones.
The adapter knows GPT-5.6 Sol's context/output limits; other discovered models use
the configured context/output budgets unless their limits are known. These fallback
budgets are not a claim about the provider's actual limits. Named profiles can set
budgets explicitly. Discovery IDs are namespaced using stable provider/model hashes.

`output_token_parameter` now defaults to `auto`: OpenAI uses
`max_completion_tokens`, SiliconFlow uses `max_tokens`, and generic endpoints use
`max_tokens` except recognized GPT-5/o-series IDs. Both `[llm]` and named profiles
can explicitly choose `max_tokens` or `max_completion_tokens` for custom endpoints.
Only one output limit field is sent.

## Model thinking mode

Configure `reasoning_effort` under `[llm]` or a named profile. It expresses intent;
the adapter chooses the wire parameters. The removed `enable_thinking` configuration
key is replaced by an effort string; SiliconFlow's wire protocol still uses its
own `enable_thinking` field.

- OpenAI GPT-5.6 Sol: Off, Low, Medium, High, XHigh, Max. Off sends
  `reasoning_effort: "none"`; other levels use their native names.
- SiliconFlow's documented DeepSeek V4/V4-Flash and GLM-5.2 models: Off, High, Max.
  Off sends `enable_thinking: false` without effort; enabled requests send
  `enable_thinking: true` and `reasoning_effort`.
- Known non-reasoning GPT-4/3.5 models expose no levels and omit reasoning fields.
- Unknown models expose no levels automatically. An explicit `reasoning_effort`
  opts in that one level. For additional verified levels, configure
  `capabilities.reasoning` with `levels`, `default`, and `source`.

Web always displays selectable **Off, Low, Medium, High, Xhigh, Max, Ultra**.
Selecting a model resets thinking to **Off**, including selecting a previously used
model. Users can then choose any grade manually, independently of capability
metadata. Manual grades pass through the adapter without remapping or silent
fallback; a provider may reject an unsupported grade. Off uses a declared disable
parameter when available; otherwise reasoning parameters are omitted. This cannot
turn off internal reasoning in providers that always enable it.
Loading and failed requests have separate states with retry.
A verified custom model may declare a restricted set without enabling every grade:

```toml
[llm.capabilities.reasoning]
levels = ["high", "ultra"]
default = "high"
source = "configuration"
```

For named profiles, use `[models.profiles.<profile>.capabilities.reasoning]`.

`GET`/`PUT /v1/sessions/{id}/model` return the model and its `thinking` state from
one immutable catalog snapshot. Thinking state includes `status`
(`supported`, `unsupported`, `unknown`), `source`, `profile`, `revision`, and the
effective level and selectable manual values. Provider-declared capabilities remain
in the model metadata and are not expanded by a manual selection. Frontend requests are cancelled or ignored
after session changes, and mixed model/capability generations are rejected.
`PUT /v1/sessions/{id}/thinking` accepts a `level`, the legacy `enabled` boolean,
or `use_default: true`. Responses include `follows_default`.

Preferences are scoped to the session, provider connection, endpoint, model and
wire protocol; credentials are excluded so rotation does not reset user choices.
Page reloads retain the manual choice, but each explicit model selection resets
it to Off. The default without a saved choice is Off.
Ambiguous older provider/model-only preference files are retained
but are not applied to a newly scoped connection. Legacy session-wide preferences
still migrate once to the currently selected model. Active executions keep their
immutable snapshot. Unsupported requests are rejected rather than silently mapped
to another grade or retried as generation requests.

Provider-returned reasoning remains separate from answer text. SiliconFlow retains
matching current-turn tool reasoning; generic endpoints can opt in using
`capabilities.reasoning_content = true`. Prior-turn reasoning is not resent.

See the official [OpenAI GPT-5.6 Sol model](https://developers.openai.com/api/docs/models/gpt-5.6-sol),
[SiliconFlow request parameters](https://docs.siliconflow.cn/docs/api/chat-completions-post),
and [SiliconFlow model discovery](https://docs.siliconflow.cn/docs/api/models-get).

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

## Gateway access

HTTP access now requires issued gateway tokens for all listeners, including loopback.
Run `kyler configure` to provision the managed service credential. Old shared tokens
and the old server-token environment variable are not imported. See [gateway identity
and authorization](gateway.md) for roles, grants, rotation, audit and upgrade behavior.

### Web provider management

Open **Settings → Providers** to add, edit, test, or delete provider connections.
The provider type selector uses the same presets as the CLI. Names identify connections
in the chat composer, where the first menu level is the provider and the second is the model.
Provider names accept letters, numbers, underscores, and hyphens.

Connection tests query the provider's model directory without sending inference requests.
Some services do not support model listing; a failed directory test does not necessarily
mean that manually configured model IDs cannot be used. Failed discovery connections
are omitted from the web model picker until discovery succeeds again.

Provider management requires an administrative identity and a trusted configuration source
registered by the host. Changes are saved atomically and loaded without a server restart.
API keys are never returned by the management API; leaving a key blank preserves it.
Setting a provider as default uses its first configured model for new conversations.
Choose another default before deleting the current default provider or model.
Existing conversations retain their selected profiles and need a new selection if those
profiles are removed. Plugins and automation configuration remain separate.

Provider API endpoints: `GET /v1/providers`, `PUT /v1/providers/{id}`,
`DELETE /v1/providers/{id}`, and `POST /v1/providers/{id}/test`.

### Web subagent settings

Open **Settings → Subagents** to enable delegation and configure its concurrency and
per-execution task limits. The supported ranges are 1–32 concurrent tasks and 1–64
tasks per parent execution. The page shows the active runtime separately from saved
configuration and indicates when a restart is required. Changes are validated and
saved atomically; restart Kyler to apply them. Running tasks retain their existing limits.
Queue, retention, and other advanced settings are preserved. If child session persistence
is enabled, disable `subagent.persist_sessions` in the server configuration before
turning delegation off. Concurrency must also fit the configured retained-task limit.

`GET /v1/subagents/settings` and `PUT /v1/subagents/settings` require an administrative
identity and a trusted configuration source. The update body contains `enabled`,
`max_concurrent_tasks`, and `max_tasks_per_execution`; responses include configured and
active values, supported ceilings, and `restart_required`. No credentials are returned.

### Review Agent skill

**Review Agent** is a bundled skill with invocation name `review-agent`.
Select it from the Web or TUI skill list, or write `@review-agent` / `$review-agent`
followed by the review request. Its package source is `builtin://review-agent`;
it is embedded in the binary rather than installed into the user's home directory.
Its reference files are read through the ordinary skill-resource mechanism.

The skill organizes review targets, workspace-relative file/directory selectors,
and reporting. For example: `@review-agent Review uncommitted changes in web/`.
Selectors are passed as `review.paths` to the existing delegated read-only executor;
they do not authorize access outside the workspace. Commit and local base-branch
reviews remain available. The executor retains immutable evidence capture, restricted
tools, structured report validation, and stale-evidence checks. It never applies fixes.
Direct review commands remain compatible with the same execution mechanism.

To customize the instructions and reference files, create a user/project skill
named `review-agent` using the normal skill installation and review flow. Package
resources use relative paths, distinct from the code paths selected for review.
An optional `metadata.display-name` in `SKILL.md` controls the label shown in Web and
TUI; the lowercase `name` remains the identifier used in calls and approval records.
