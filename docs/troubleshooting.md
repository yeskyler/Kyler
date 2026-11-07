# Troubleshooting

Start with the version, service status, and the relevant log. Redact model keys,
backend tokens, Web passwords, and private tool output before sharing diagnostics.

```sh
kyler --version
kyler service status
```

Backend and Web logs normally live at `~/.kyler/logs/server.log` and
`~/.kyler/logs/web.log`. A custom configuration path changes the service-record
and log base directory.

## Command or manifest not found

If `kyler` is not on PATH after installation:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

`--modify-path` configures new terminals. Rust commands run from `kyler-rs/` or
use an explicit manifest:

```sh
cargo build --manifest-path kyler-rs/Cargo.toml --release -p kyler-cli
```

After moving a checkout, reload the editor's workspace if its language server
still references previous Cargo paths. A missing example target is a manifest or
source-path problem; downloading rust-analyzer again does not repair that target.

## Missing model key or provider errors

Run `kyler configure --web` for a complete installation, then restart services.
Check the selected configuration and any process environment overrides.
Model name and image capability settings must match the deployed provider.
A rejected key, connection error, or unsupported image response is a provider-path
failure, not evidence of missing local tools.

## Hidden input initialization failure

Older installer builds used a terminal event reader that could fail at the hidden
API-key prompt on macOS. Current Unix setup reads hidden input without that event
reader. Rebuild/repackage the fixed CLI and retry the newly generated bundle;
reusing an older archive continues to run its embedded binary.
Normal terminals retain standard input; piped installers use the controlling
terminal when available. Without an interactive terminal, use saved values and
`--non-interactive` with credentials supplied securely through the environment.

## Address occupied or identity unverified

Choose distinct backend and Web listen addresses during setup. Use
`kyler service status` to identify managed services before restarting. The manager
will not terminate a process whose identity it cannot verify. Do not delete
records or kill arbitrary port owners as a first step.

If another application owns a port, stop that application intentionally or select
a different port. A wildcard listen address such as `0.0.0.0` is not a browser
connection URL; use localhost or the machine's reachable address.

## Web missing, stale, or disconnected

A backend-only `--local` install does not contain Web. Use a complete bundle.
For a managed complete installation, use `kyler service start --restart` after
configuration changes. For manual Next.js production serving, run `npm run build`
from `web/` after source changes; `npm run start` serves the existing build.

Check `client.server_url`, backend authentication, and the Web log. A public Web
binding needs its configured access password. Public backend bindings also restrict
workspace selection. See [configuration](configuration.md) and
[installation](installation.md).

## Tool execution failures

A discovered tool is not proof that its dependencies or external service are ready.
Inspect the tool's recorded error, arguments, sandbox permissions, network policy,
and service availability. An MCP connector can initialize successfully while its
own upstream API remains unreachable.

For commands, distinguish an unavailable executable from an unavailable interpreter
or dependency. For patches, inspect patch syntax and current file content.
Use the configured approval interaction when approval is requested; approval cannot
exceed host permissions. Do not infer that Rust or Python is absent solely from a
sandbox spawn error.

## Upgrade failure

Check the installer output and both logs. Complete upgrades attempt to restore the
previous version and configuration on startup failure. If cleanup or identity
verification cannot complete, inspect the recorded services before retrying rather
than forcing removal. Version directories are retained for recovery.

For evaluation-specific issues, see [evaluations](evaluations.md).
