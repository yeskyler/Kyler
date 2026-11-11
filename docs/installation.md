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

The installer is hosted on the installation server, separately from Release assets.
It selects
the latest published non-prerelease version and detects the
host platform, downloads the corresponding archive and `SHA256SUMS`, and validates the
archive before installation. `--version VERSION` selects a specific published release.
The required platform archive must be present in that release.

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

### Service addresses

| Input | Default |
| --- | --- |
| Backend listening address | `127.0.0.1:30001` |
| Web listening address | `127.0.0.1:30002` |

Listening addresses use a local IP address and a fixed port. IPv6 addresses use
brackets. Wildcard listeners are supported; public domains and HTTPS URLs are configured
at a reverse proxy. The backend and Web listeners must not overlap.

Setup derives the Web-to-backend connection URL from the backend listener and uses
loopback for wildcard addresses. A separately hosted backend is configured through
`[client].server_url` and `[client].access_token` in the configuration file.

A non-loopback Web listener requires an access password. Browser authentication uses
HTTP Basic with username `kyler` and the configured password. Backend credentials remain
on the Web server. HTTPS must terminate at a reverse proxy when the service is exposed
over an untrusted network.

Non-loopback backend services restrict project selection to the configured default
workspace. Local project selection requires a loopback backend listener.

### Service status and management

After successful activation, the installer displays backend and Web status, connection
addresses, the configuration path, and service management commands. The default browser
address is **http://127.0.0.1:30002**.

| Operation | Command |
| --- | --- |
| Inspect status | `kyler service status` |
| Stop services | `kyler service stop` |
| Start services | `kyler service start` |
| Restart services | `kyler service start --restart` |
| Reconfigure model and addresses | `kyler configure --web` |
| Open the terminal interface | `kyler tui` |

Restart services after changing the saved configuration. Installation automatically
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
