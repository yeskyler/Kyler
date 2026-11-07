# Troubleshooting

Record the installed version, service status, and relevant log entries before diagnosing
a failure. Redact model keys, backend tokens, Web passwords, and private tool output
before sharing diagnostics.

```sh
kyler --version
kyler service status
```

Backend and Web logs are stored at `~/.kyler/logs/server.log` and
`~/.kyler/logs/web.log`. A custom configuration path changes the service-record and log
base directory.

## Command or manifest not found

If `kyler` is not on PATH after installation:

```sh
export PATH=~/.local/bin:"$PATH"
```

`--modify-path` configures new terminals. Rust commands run from `kyler-rs/` or use an
explicit manifest:

```sh
cargo build --manifest-path kyler-rs/Cargo.toml --release -p kyler-cli
```

After moving a checkout, reload the editor's workspace if its language server still
references previous Cargo paths. A missing Cargo target requires correction of the
manifest or source path. Reinstalling the language server does not resolve an invalid
target definition.

## Release download returns 404

Verify that the release is published, that `install.sh` and the required platform
archive are attached, and that the repository is accessible to the downloading account.
Private release URLs require authentication. Download the authorized assets and install
with `--bundle` when anonymous download is unavailable.

## Missing model key or provider errors

Run `kyler configure --web` for a complete installation, then restart services. Check
the selected configuration and any process environment overrides. Model name and image
capability settings must match the deployed provider. Authentication rejection,
connection failures, and unsupported image responses require inspection of the provider
endpoint, credentials, and model capabilities.

## Interactive input unavailable

Interactive setup requires a terminal. Standard invocation reads from standard input;
piped invocation uses the controlling terminal when available. API-key input is hidden
and terminal settings are restored after input or cancellation.

Without an interactive terminal, use `--non-interactive` with saved configuration or
credentials supplied through process environment variables. A local bundle contains its
own CLI binary; rebuilding source does not update an existing archive.

## Address occupied or identity unverified

Choose distinct backend and Web listen addresses during setup. Use `kyler service
status` to identify managed services before restarting. The manager will not terminate a
process whose identity it cannot verify. Retain service records until process ownership
and identity have been established.

If another application owns a port, stop that application intentionally or select a
different port. Wildcard listen addresses (`0.0.0.0` and `[::]`) require a separate
browser connection address: loopback for local access or a reachable host address for
remote access.

## Web missing, stale, or disconnected

A backend-only `--local` install does not contain Web. Use a complete bundle. For a
managed complete installation, use `kyler service start --restart` after configuration
changes. For manual Next.js production serving, run `npm run build` from `web/` after
source changes; `npm run start` serves the existing build.

Check `client.server_url`, backend authentication, and the Web log. A public Web binding
needs its configured access password. Public backend bindings also restrict workspace
selection. See [configuration](configuration.md) and [installation](installation.md).

## Tool execution failures

Tool discovery and dependency availability are separate checks. Inspect the tool's
recorded error, arguments, sandbox permissions, network policy, and service
availability. An MCP connector can initialize successfully while its own upstream API
remains unreachable.

For commands, distinguish an unavailable executable from an unavailable interpreter or
dependency. For patches, inspect patch syntax and current file content. Use the
configured approval interaction when approval is requested; approval cannot exceed host
permissions. A sandbox process-start error also requires inspection of host containment
support and executable permissions.

## Upgrade failure

Check the installer output and both logs. Complete upgrades attempt to restore the
previous version and configuration on startup failure. If cleanup or identity
verification cannot complete, inspect the recorded services before retrying. Preserve
recovery records until service ownership is verified. Version directories are retained
for recovery.

For evaluation-specific issues, see [evaluations](evaluations.md).
