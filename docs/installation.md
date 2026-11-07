# Installation

The complete release includes Kyler, the Web UI, and a Node.js runtime. End users
do not need Rust, Cargo, npm, or a separate Node.js installation. macOS and Linux
are supported; use a release built for your operating system and CPU.

Run repository commands below from the repository root.

## Install a published release

Download `install.sh` from [GitHub Releases](https://github.com/yeskyler/Kyler/releases)
and run it locally:

```sh
sh install.sh --modify-path
```

Use `--version 0.1.0` to select an available version. The installer downloads the
native archive for this machine and verifies its checksum. This requires the
corresponding release assets to have been published. Downloading and installing
an existing local archive is also supported below.

## Install a local release

```sh
sh scripts/install.sh --bundle ./dist/kyler-v0.1.0-aarch64-apple-darwin.tar.gz --modify-path
```

Setup asks for each value in order. Enter keeps the current value; credentials
are hidden. Esc cancels setup before replacing an existing installation.

| Setting | Default |
| --- | --- |
| Model | `deepseek-ai/DeepSeek-V4.1-Flash` |
| Model API URL | `https://api.siliconflow.cn/v1` |
| Model API key | Required |
| Backend listen address | `127.0.0.1:30001` |
| Backend connection URL | Derived from the backend listen address |
| Web listen address | `127.0.0.1:30002` |

Both listening addresses are configurable, including the IP and port. Examples:
`127.0.0.1:31001`, `0.0.0.0:31002`, `[::1]:31002`. A listen address must be an IP
on this machine (or a wildcard); it is not a public domain or HTTPS URL.
For a wildcard backend address, setup suggests a loopback connection URL rather
than trying to connect to `0.0.0.0`. The connection URL can also be changed to an
HTTP/HTTPS backend endpoint; a custom endpoint can have its own access token.

After installation, open **http://127.0.0.1:30002**, or the Web address you chose.
Both services start in the background by default. In the current terminal:

```sh
export PATH="$HOME/.local/bin:$PATH"
kyler service status
kyler service stop
kyler service start
kyler service start --restart
```

For the terminal interface, run `kyler tui` in your project directory.
To change the installed model and addresses, run `kyler configure --web`, then
`kyler service start --restart`.

When Web listens beyond loopback, setup requires a Web access password. The
browser uses HTTP Basic authentication (username `kyler`, configured password).
Put HTTPS at a reverse proxy when exposing the service over an untrusted network.
Backend credentials stay on the Web server; they are not sent to the browser.
Non-loopback backend service launches restrict workspaces to the configured
default workspace. Use a loopback backend behind the Web server for local folder
selection and access to other local project directories.

## Files and upgrades

```text
~/.local/bin/kyler                         command entry
~/.local/share/kyler/versions/<version>/    backend, Web UI, Node runtime
~/.local/share/kyler/current               active version symlink
~/.kyler/config.toml                      private configuration
~/.kyler/logs/server.log                   backend log
~/.kyler/logs/web.log                      Web log
~/.kyler/workspace/                        default workspace
```

Run the same bundle install command to upgrade. Configuration, sessions, skills,
and workspace files are preserved. The installer checks checksums, prepares a
candidate configuration, and only then switches versions. If startup fails, it
attempts verified cleanup and restores the previous binary, configuration, and
previously running services. If safe cleanup cannot be verified, it reports the
failure for inspection rather than stopping an unrelated process.
Unused version directories are retained for recovery.

`--no-start` installs without starting services. `--no-configure` keeps the
configuration as-is; an existing backend-only configuration must first enable
`[web].enabled = true` to start Web. `--non-interactive` uses saved values and
environment variables, including `KYLER_LLM_MODEL`, `KYLER_LLM_BASE_URL`,
`KYLER_LLM_API_KEY`, `KYLER_SERVER_BIND`, `KYLER_WEB_BIND`,
`KYLER_CLIENT_SERVER_URL`, and `KYLER_WEB_TOKEN`.

Backend-only local installs remain supported:

```sh
cargo build --manifest-path kyler-rs/Cargo.toml --release -p kyler-cli
sh scripts/install.sh --local ./kyler-rs/target/release/kyler --modify-path
```

Use `--bundle` to upgrade a complete installation; `--local` does not include Web.

## Download from a published release

Once release assets are available, download the release's `install.sh` and run:

```sh
sh install.sh --modify-path
sh install.sh --version 0.1.0 --modify-path
```

The script selects the native platform archive from `yeskyler/Kyler`, downloads
it, and verifies its checksum. `KYLER_REPOSITORY=owner/repository` selects another
release repository. Availability depends on the maintainer publishing the assets.

See [configuration](configuration.md), [troubleshooting](troubleshooting.md), and
[release packaging](releasing.md) for details.
