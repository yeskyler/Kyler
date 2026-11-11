# Installation

A complete Kyler bundle contains the backend, Web application, TUI, CLI, and a native
Node.js runtime. Installation does not require Rust, Cargo, npm, or a separate Node.js
installation. The archive must match the host operating system and CPU architecture.

Run source-repository commands from the repository root.

## Release installation

Download and execute the installer:

```sh
curl -fsSL http://www.kyler.work/install.sh -o install.sh
sh install.sh
```

The installer is hosted separately from Release assets. It reads the latest version
from `https://www.kyler.work/releases/latest.txt`, detects the host platform, and
fetches the archive and `SHA256SUMS` from the server's `/releases/vVERSION/` directory.
If version discovery fails, it queries GitHub Releases. If a server download fails,
it downloads both files again from GitHub for the same version. Checksum failures
stop installation; they do not silently switch sources. `--version VERSION` skips
latest-version discovery.

Six numbered steps cover environment checks, download, verification, installation,
configuration, and service startup. Downloads show the source, file, size, speed, and
remaining time. Failed downloads display manual download and local installation
commands. Local bundles skip release downloads.

Verified downloads are kept in `${XDG_CACHE_HOME:-$HOME/.cache}/kyler/downloads/vVERSION/`.
Before downloading an archive, the installer fetches the selected source's current
checksums and checks both this cache and the current directory. Matching archives
are reused; corrupt or replaced versions are downloaded again. The cache survives
configuration or startup failures.

On Linux, environment checks require Bubblewrap 0.5.0 or newer. A compatible installed
version is reused. On apt-based systems, missing or outdated Bubblewrap is installed
using `sudo apt update` and `sudo apt install -y bubblewrap` (without sudo when root),
then checked with `bwrap --version`. Other distributions receive manual installation
instructions. Host user-namespace restrictions can still prevent sandbox startup.

Unauthenticated download URLs require a publicly accessible repository. Private release
assets require authenticated download followed by local bundle installation.

## Local bundle installation

Build and install a native bundle from source:

```sh
sh scripts/package.sh
```

On macOS Apple Silicon, the generated archive is installed with:

```sh
sh scripts/install.sh --bundle ./dist/kyler-v0.1.0-aarch64-apple-darwin.tar.gz
```

Archive names follow `kyler-v<version>-<target>.tar.gz`. The target identifies the
operating system and architecture. [Release management](releasing.md) defines packaging
requirements and artifact validation.

## Configuration stages

The installer completes three stages in sequence. Each stage advances after its required
inputs pass validation; no additional navigation confirmation is required.

### Model configuration

| Input | Requirement or default |
| --- | --- |
| `OPENAI_BASE_URL` | `https://api.siliconflow.cn/v1`; OpenAI-compatible Chat Completions API |
| `OPENAI_API_KEY` | Required; input is hidden |
| Model | `deepseek-ai/DeepSeek-V4-Flash` |

Enter retains an existing value. A saved key is identified as configured without
revealing its contents. Invalid input is rejected at the current prompt. Setup does not
send a model request. Esc cancels configuration before an existing installation is
replaced.

### Access scope and Web token

Setup asks for one access scope, not individual addresses or a password:

| Choice | Backend listener | Web listener |
| --- | --- | --- |
| This machine only (default) | `127.0.0.1:30001` | `127.0.0.1:30002` |
| Local network | `0.0.0.0:30001` | `0.0.0.0:30002` |

Existing ports and advanced environment overrides are retained. Reconfiguration
selects the current scope by default. Occupied ports are reported before saving.
The Web-to-backend connection uses loopback for wildcard listeners; browsers
continue to use the same-origin Next.js API proxy.

Setup generates a Web token automatically and preserves it across reinstalls and
scope changes. Both modes use the same token sign-in page. Paste the token into
that page; successful sign-in sets a signed, HttpOnly, host-only cookie valid for
seven days. Tokens are not included in URLs. TUI needs no Web token.

View the token with `kyler service token`. Rotate it with
`kyler service token --reset`; existing browser sessions fail their next protected
request. Managed Web services reload the saved token without restarting. Unset
`KYLER_WEB_TOKEN` before rotating a saved token; manually launched development
servers using that override need their environment updated as well.

Backend API credentials remain separate, server-side, and are generated automatically
for non-loopback listeners. HTTPS can terminate at a reverse proxy for access across
untrusted networks.

Non-loopback backend services restrict project selection to the configured default
workspace. Local project selection requires a loopback backend listener.

### Service status and management

After successful activation, the installer displays backend and Web status, connection
addresses grouped by interface in network mode, the Web token, the configuration path,
and service management commands. Interface enumeration makes no external IP requests. The default browser
address is **http://127.0.0.1:30002**.

| Operation | Command |
| --- | --- |
| Inspect status | `kyler service status` |
| Stop services | `kyler service stop` |
| Start services | `kyler service start` |
| Restart services | `kyler service start --restart` |
| View Web token | `kyler service token` |
| Rotate Web token | `kyler service token --reset` |
| Reconfigure model and access scope | `kyler configure --web` |
| Open the terminal interface | `kyler tui` |

Restart services after changing listener settings. Web token rotation takes effect
without restarting. Installation automatically
updates the shell startup file for new terminals. To use the command in the current shell:

```sh
export PATH=~/.local/bin:"$PATH"
```

## Installation paths and upgrades

| Path | Contents |
| --- | --- |
| `~/.local/bin/kyler` | Command entry point |
| `~/.local/share/kyler/versions/<version>/` | Versioned backend, Web application, and Node.js runtime |
| `~/.local/share/kyler/current` | Active-version symbolic link |
| `~/.kyler/config.toml` | Private application configuration |
| `~/.kyler/logs/server.log` | Backend log |
| `~/.kyler/logs/web.log` | Web log |
| `~/.kyler/workspace/` | Default project directory |

Reinstall a complete bundle to upgrade. The installer validates checksums and prepares a
candidate configuration before activating the new version. Existing configuration,
sessions, skills, and project files are preserved.

If startup fails, the installer attempts to stop the new services and restore the
previous installation, configuration, and service state. When cleanup cannot be
verified, installation reports the failure for inspection. Version directories remain
available for recovery. [Troubleshooting](troubleshooting.md) covers service diagnostics
and recovery.

## Installation options

Run `sh install.sh` to install the latest release, configure the application, and start
both services. The installer automatically configures PATH for new terminals.

| Option | Behavior |
| --- | --- |
| `--bundle PATH` | Install a complete local archive or bundle directory |
| `--version VERSION` | Select a published release version |
| `-h`, `--help` | Display installation help |

Advanced options support automation and maintenance:

| Option | Behavior |
| --- | --- |
| `--no-start` | Install without starting services; existing managed services are stopped during upgrades |
| `--no-configure` | Skip the configuration wizard; create defaults only when configuration is absent |
| `--non-interactive` | Use saved configuration and process environment without prompts |

Non-interactive setup accepts the environment overrides listed in
[configuration](configuration.md). Installations always include the backend, Web UI,
and Node.js runtime. `KYLER_REPOSITORY` selects the GitHub repository used for release
downloads.

### Local terminal agent

Run `kyler tui` from your project directory. It loads local model credentials and
executes the agent in the terminal process; backend and Web services do not need
to be running. The backend access token and `--server` option are not used by TUI.

Resume a local session with `kyler tui --session <session-id>`. Local TUI sessions
are separate from Web sessions and normally live in `~/.kyler/tui/sessions`.
The saved workspace must still exist and match the configured sandbox policy.

Reasoning streams in a muted preview showing the latest six display lines. It
collapses by default when the agent starts an answer or tool call. Click its
heading or press `Ctrl+O` to toggle the latest reasoning block; expanding retains
the full text. A manual disclosure choice stays in effect as new fragments arrive.
Completed live blocks show their elapsed thinking time. Restored history starts
collapsed and does not invent timing information that was not saved.
