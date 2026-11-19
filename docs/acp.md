# ACP editor integration

Kyler implements the Agent side of stable Agent Client Protocol v1 using the
[official Rust SDK](https://agentclientprotocol.com/libraries/rust), pinned to
`agent-client-protocol` 5.4.0. It runs locally without starting the HTTP/Web server.

## Launch

Configure an ACP-capable editor to launch:

```sh
kyler --config /absolute/path/to/config.toml acp
```

The command requires the same validated local model configuration as TUI.
`--server` and `--json` are rejected. Standard output is reserved for JSON-RPC;
diagnostics and startup errors go to standard error. Model credentials are read
from host configuration and are never sent to the editor.

## Supported flow

- `initialize`: stable v1 negotiation and conservative capability advertisement.
- `session/new`: an absolute directory and a new connection-owned session.
- `session/prompt`: text and resource links, streaming text and tool progress,
  execution-plan updates, and a final stop reason after execution cleanup.
- `session/request_permission`: allow once or deny, through the core approval API.
- `session/cancel`: cancel an active turn while keeping the protocol reader live.

Resource links become untrusted context references. They are not automatically
fetched; any subsequent file or network access uses ordinary tool permissions.
Only one prompt may run in a session. Different sessions remain independent.
The connection holds at most 128 sessions. Disconnecting cleans up its runtime.

ACP sessions are stored under `acp/sessions` in the configuration data directory,
separate from Web's `sessions` and TUI's `tui/sessions`. The core's existing workspace
factory determines filesystem authority; the editor cannot select Full access or
bypass protected paths. File and command operations run through Kyler's existing
execution environment, not through editor-provided filesystem/terminal callbacks.

## First-version limits

`session/load`, model/mode switching, images/audio, editor buffer reads, client
terminals and structured elicitation are not advertised or implemented. If the
agent requests a structured question, Kyler tells the editor and cancels that
question so the turn cannot hang waiting for an unsupported UI.

Host-configured MCP connectors remain available, including `search_tool_bm25`.
Nonempty client-supplied `mcpServers` are rejected rather than starting a supplied
executable or connecting to a supplied endpoint without host authorization.
There is no ACP credential-login flow; local model credentials must be configured
before launch. Existing event formats and Web/TUI transports are unchanged.

## Validation

Protocol tests connect both sides of the official SDK over in-memory transports.
The CLI integration test launches the real executable with a temporary HOME and
configuration, plus a loopback fake model. No paid provider is called.

```sh
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-acp
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-cli --test acp
```
