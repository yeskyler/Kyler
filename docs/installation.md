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

Six numbered steps cover system compatibility, sandbox dependencies, download and
verification, native self-checks, configuration and installation, then service startup.
Downloads show the source, file, size, speed, and remaining time. Failed downloads
display manual download and local installation commands. Local bundles skip release
downloads but still undergo compatibility and sandbox checks.

Verified downloads are kept in `${XDG_CACHE_HOME:-$HOME/.cache}/kyler/downloads/vVERSION/`.
Before downloading an archive, the installer fetches the selected source's current
checksums and checks both this cache and the current directory. Matching archives
are reused; corrupt or replaced versions are downloaded again. The cache survives
configuration or startup failures.

### Host compatibility and sandbox dependencies

The installer recognizes macOS/Linux and x86_64/ARM64. A release must contain the
matching architecture. Current Linux download targets use glibc; musl-based systems
such as Alpine are rejected before downloading and require a separate compatible
build. `/etc/os-release` is read for display, not executed. A detected glibc version
is informational: execution of both the bundled Kyler and Node.js is the final ABI
compatibility check. There is no universal minimum glibc guarantee across locally
built bundles, and the installer does not upgrade system libraries.

On Linux, Bubblewrap >= 0.5.0 and the networking tools required by the pinned MXC
adapter are checked before download: `slirp4netns`, `ip`, `unshare`, `nsenter`,
`iptables`, `ip6tables`, their `-restore` commands, `sh`, and `sleep`. Compatible
installed tools are reused. Missing dependencies use the first available supported
package manager:

| Package manager | Packages |
| --- | --- |
| `apt-get` | `bubblewrap slirp4netns iproute2 util-linux iptables` |
| `dnf` / `yum` | `bubblewrap slirp4netns iproute util-linux iptables` |
| `pacman` | `bubblewrap slirp4netns iproute2 util-linux iptables` |
| `zypper` | `bubblewrap slirp4netns iproute2 util-linux iptables` |

Commands and package-manager output remain visible. Non-root users need `sudo` to
install missing packages; root invokes the manager directly. Unknown managers,
insufficient privileges, failed installations and insufficient repository versions
stop with manual instructions. No third-party repository is added. Arch installation
uses `pacman -S --needed` without a partial repository refresh; maintain the host
through its normal full-system update procedure if repository versions are outdated.
Package-manager recognition is not a certification of every distribution/version.

macOS uses the system Seatbelt sandbox without downloading a separate sandbox runtime.
Windows is not currently supported by Kyler's adapter, regardless of MXC's wider
platform support.

### Native sandbox self-check

After verification, before replacing the old installation or stopping services, the
new binary runs `kyler doctor` as the installing user. It uses temporary fixtures and
a disposable local TCP listener, without reading application configuration, contacting
a model or using an API key. Both offline and outbound sandbox policies must launch
successfully and enforce workspace writes, protected/denied/outside paths and denial
of host loopback access. A child shell is also executed inside each sandbox. Worker
execution uses the same MXC supervision and cleanup as application tools.

The check exercises network-policy setup and local isolation; it does not verify
public internet reachability or every possible destination. Linux user namespaces,
AppArmor, namespace tooling versions, firewall backends, kernel modules and container
host restrictions can prevent execution even when packages are installed. Failures
stop before activation and preserve the previous installation/configuration. The
installer never disables host security or falls back to unrestricted execution.
For a restricted container or cloud image, ask its administrator to resolve the
reported policy/kernel limitation rather than running Kyler as root to bypass it.

Run checks independently, including before starting a source build:

```sh
kyler doctor
kyler --json doctor
```

Older bundles without `doctor` cannot pass this installer; use a newly built complete
bundle. Updating only `install.sh` does not update an existing release binary.

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
local addresses plus one primary interface under `Network access` in network mode,
the Web token, the configuration path, and service management commands. The interface
is selected from active IPv4 links and the host's default routes (Linux route metric;
macOS unscoped defaults before scoped defaults). Known VPN/tunnel/container links and
reserved addresses are excluded; a scoped physical default can be selected beneath a
VPN. Equal-priority links, multiple addresses on the selected link, missing routes or
routing-table errors produce an identification message instead of an arbitrary URL.
This changes display only, not listeners or routing. No external IP service is queried,
and an interface address is not claimed to be a public IP. The default browser address
is **http://127.0.0.1:30002**.

| Operation | Command |
| --- | --- |
| Inspect status | `kyler service status` |
| Follow backend logs | `kyler logs backend` |
| Follow Web logs | `kyler logs web` |
| Stop services | `kyler service stop` |
| Start services | `kyler service start` |
| Restart services | `kyler service start --restart` |
| View Web token | `kyler service token` |
| Rotate Web token | `kyler service token --reset` |
| Reconfigure model and access scope | `kyler configure --web` |
| Open the terminal interface | `kyler tui` |

`kyler logs backend` and `kyler logs web` show the last 100 lines, then continuously
follow new output. `kyler logs` defaults to the backend. Press Ctrl+C to exit the viewer without stopping the service. The commands
read `logs/server.log` or `logs/web.log` beside the selected configuration file;
`--config` and `KYLER_CONFIG` use the same path rules as service management. No API key,
valid model configuration, or running backend is required. Missing logs produce a clear
path message. Following handles log replacement and truncation without an external
`tail` dependency. Output is the original log content; `--json` is not supported.

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
