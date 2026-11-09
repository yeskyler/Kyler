# Architecture

Kyler combines a Rust agent backend with Web, TUI, and CLI clients. The installed
`kyler` executable includes the CLI, server launcher, TUI, and internal worker entry.
The standalone Web application runs in a separate Node.js process.

## Request path

```mermaid
flowchart TD
    Browser[Web browser] --> Next[Next.js server and API proxy]
    Next --> Server[Rust HTTP and SSE server]
    TUI[TUI and CLI] --> Client[kyler-client]
    Client --> Server
    Server --> Runtime[kyler-core runtime]
    Runtime --> Model[Configured model provider]
    Runtime --> Tools[Tools and integrations]
    Tools --> Worker[Sandbox worker and managed processes]
    Server --> Store[Session store and Rollout]
```

Web keeps provider and backend credentials server-side. HTTP requests and SSE carry
structured state and events; clients render text, tool activity, approvals, questions,
and artifacts through their respective views.

## Rust modules

| Crate | Responsibility |
| --- | --- |
| `kyler-cli` | Command parsing, setup, service lifecycle, and launch entry points |
| `kyler-server` | HTTP routes, SSE, run admission, cancellation, workspace binding |
| `kyler-tui` | Terminal interaction, input, and structured history rendering |
| `kyler-client` | Rust HTTP/SSE transport and typed client operations |
| `kyler-core` | Runtime, tools, sessions, context, memory, sandbox, integrations |
| `kyler-protocol` | Shared contracts, event metadata, prompts, and agent names |
| `kyler-config` | Configuration loading, defaults, validation, credential resolution |
| `kyler-logging` | Logging configuration and initialization |
| `kyler-evals` | GAIA evaluation and startup measurement executables |

Rust crates are located in `kyler-rs/crates/`. Web source is located in `web/` and
mirrors the public contracts in its frontend types and event adapters.

## Runs, sessions, and history

A session associates conversation state with a project identity and a private execution
workspace. A run executes one admitted request, with its own execution state and
cancellation. Different sessions may run concurrently within server limits; overlapping
turns in the same session are restricted to preserve history ordering.

The runtime prepares a model request, receives text/tool calls, validates and executes
tools, records their results, and continues until completion or a configured execution
limit is reached. Managed commands can return a process ID while still running.
Subagents are optional independent child sessions managed by delegation tools; a child
does not recursively inherit subagent-management tools.

Persistent sessions use an append-only `rollout.jsonl` journal and snapshots for loading
and recovery. TUI and Web consume shared structured history rather than reconstructing
tool state from plain assistant text.

## Deliverables

The `register_artifacts` tool replaces the current execution's deliverable list with
explicit file paths, display names, and at most one primary file. Registration validates
workspace access and the per-file size limit without granting additional permissions.
At successful completion, the server retains immutable snapshots before publishing the
terminal run event. Registration state belongs to one execution and does not carry
forward into later turns.

Registered files define the output list. When no files are registered, local links in
the final response remain supported for compatibility. The host does not scan workspace
directories for deliverables. Local dependencies of retained HTML and Markdown files
remain hidden from the top-level list. Web opens an available, supported primary file automatically;
other files remain selectable from the browser. Missing primary files never cause an
unrelated file to be selected automatically. File-loading failures show an error and a
retry control.

XLSX and PPTX previews use `@vue-office/excel` and `@vue-office/pptx` in read-only,
opaque-origin iframes. The Web build bundles the components, Vue runtime, compatibility
layer, and styles as local static assets. The parent validates file size and ZIP
expansion limits before passing bytes to each frame; it does not pass service
credentials. Frames permit local blob reads and block remote resource connections.
Removing a frame releases the component's global event handlers and document resources.

PPTX preparation runs in a Worker and removes dangling content-type declarations from
a preview copy, preserving the original download. Previews support at most 100 slides
and 20 MiB of compressed input. Page navigation and scaling reuse rendered slides and
preserve the presentation's page ratio. Advanced chart styling, animations, and embedded
media depend on the renderer's capabilities.

## Context and memory

Context management builds a bounded model-facing view, preserves the current question,
and compacts history according to the configured window and summary policy. Large tool
outputs can have a compressed model view while retaining their original output in
history.

Text estimates use `tiktoken-rs`, with `o200k_base` as the default encoding. Model
profiles can explicitly select `cl100k_base`. The legacy `conservative` setting is
accepted as an alias for `o200k_base`; there is no byte-counting fallback. These
encodings remain estimates for providers using different tokenizers. Provider-reported
input usage takes precedence in the context inspector when available. Images retain
a separate visual-token allowance.

Context compaction records a durable lifecycle with a stable identifier, timestamps,
and status. The service streams these updates through `context_compaction` events
and includes them in session history. Web and TUI render one separator per lifecycle,
updating its status when compaction completes, fails, or is cancelled. History replay
marks unfinished lifecycles as interrupted rather than leaving an active indicator.
These records contain no summary text and are excluded from model requests and
summary input.

Session summaries are part of session state. Long-term project facts are explicitly
managed by the `memory` tool and stored separately by the service, scoped to the
resolved project/session identity. Only explicit memory operations create or update
these facts. Rollout, Memory, and Context inspectors expose different views of this
state.

## Trust and execution

The host creates an execution environment with a workspace, working directory,
filesystem grants, network policy, process limits, and a worker path. Tools declare
execution placement and operation metadata. Tool dispatch validates arguments, checks
rules and approval, and executes under the effective host-authorized policy.

Isolated file/command work uses the sandbox worker. Host-managed processes retain
session ownership while command execution uses the worker boundary. MCP tools use
session-bound connections and still depend on remote credentials and permissions. Skills
provide instructions and resources; enabling a skill does not increase authority.

Product instructions are maintained in `kyler-rs/crates/kyler-protocol/src/prompts/`,
separate from the repository's [AGENTS.md](../AGENTS.md). See
[configuration](configuration.md), [development](development.md), and [security
guidance](../SECURITY.md).

## Private session execution

The workspace selected by a client is a project grouping, not a shared execution root.
Each managed session receives a private directory under
`<session.directory>/_execution/<session-id>/workspace/`. Its serialized environment
stores the grouping in `project_root`, while `workspace_root` and `working_directory`
both identify the private execution directory. Public session summaries continue to
expose the project grouping for navigation.

The server derives the private path from the owning session ID, checks it on restore,
and restores the host policy before narrowing filesystem access. Built-in file tools,
patch paths, working directories and native command containment all use the private
root. Global extra filesystem roots do not become shared session access; host-installed
runtime paths remain available read-only. A session cannot select unrestricted execution
to bypass this boundary. Subagents belonging to a parent task inherit that parent's
private execution environment.

Existing sessions are narrowed when continued. History, uploads and retained artifact
copies are preserved; only that session's retained output snapshots are restored into
its private directory. Public project files are neither copied nor moved. New private
workspaces start empty. Task inputs are supplied through session attachments or created
and retrieved within the owning session. An absent private directory or a mismatched
owner fails closed instead of rebinding to the public workspace. Session deletion
removes the owning session's private execution directory, records, and attachments.
Project directories and other sessions remain unchanged.

## Tool progress events

After recording a model's complete tool-call batch, the runtime emits `tool_queued` for
every call before authorizing and executing calls in order. `tool_started` means
execution has begun; `tool_finished` also covers calls refused by approval. Web and TUI
update existing nodes by call ID, distinguish queued and approval waits, and settle
unfinished nodes when a run ends. History uses retained presentation timestamps to
distinguish calls that have not started.

The Web client loads successful plan-tool details independently of the SSE event loop,
with a timeout and protection against older results replacing newer plans. Tool
execution remains serial within each batch.
