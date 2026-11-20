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

The shell bootstrap only obtains and verifies a native CLI executable, then hands
control to `kyler install`. All bundle downloading, archive extraction, integrity
checks, native sandbox checks, activation and configuration UI run in Rust. The
bootstrap uses one brief preparation line. Installation groups its checks into
**Prepare package**, **Check environment**, and **Install Kyler**, then opens
configuration and launch. All stages share inline `indicatif` progress and
`cliclack` setup controls. Diagnostics are saved to
private installation logs; failures show a concise reason and the log path.

Packages also provide `kyler-installer-vVERSION-TARGET`, covered by the release's
`SHA256SUMS`. This currently reuses the full CLI executable rather than maintaining
a separate installer engine. Keep that executable, `SHA256SUMS` and the archive
together for offline use. If absent, the bootstrap downloads a verified installer.
For source development or a trusted offline installer, set `KYLER_INSTALLER_BINARY`
to its executable path. This explicit override is trusted executable code, not a
replacement for verification of the selected bundle.

An interactive installation opens `kyler configure` as the final step, after
activation and PATH setup. Choose **Save and start** or **Save without starting**.
Cancelling setup keeps the installed program; run `kyler configure` to resume.
On upgrades with valid saved settings, choose **Keep current configuration and start**
to avoid model discovery and repeated credential entry, or **Edit configuration**.
Running `kyler configure` directly always opens the editing flow.
Non-interactive installs skip this wizard unless `--configure` is explicitly requested. Upgrades preserve existing configuration and restore
previously running services; stopped services remain stopped. During interactive
upgrades, restoration waits until configuration finishes. Choosing **Save without
starting** leaves services stopped; cancelling setup restores previously running
services using their saved configuration. For explicit automation,
use `--configure --start` (with `--non-interactive` when appropriate).
Local bundles skip downloading but retain integrity and sandbox verification.

Verified downloads are kept in `${XDG_CACHE_HOME:-$HOME/.cache}/kyler/downloads/vVERSION/`.
Before downloading an archive, the installer fetches the selected source's current
checksums and checks both this cache and the current directory. Matching archives
are reused; corrupt or replaced versions are downloaded again. The cache survives
configuration or startup failures.

### Host compatibility and bundled runtime

Release bundles contain their own ripgrep. Kyler never selects a host `rg`, even
when one is installed. macOS uses a standalone native build without Homebrew/PCRE2
dylib dependencies and the operating system's Seatbelt backend.

Linux bundles also contain Bubblewrap >= 0.5.0, slirp4netns, ip, unshare, nsenter,
iptables/ip6tables and their restore commands, sh, sleep, xtables extensions, and
the required non-glibc shared libraries. Private launchers scope library/module
paths to the package. SDK helpers must resolve to this payload; missing, damaged
or shadowed payloads fail instead of falling back to system tools.

The installer does not invoke brew, apt, sudo or another package manager, and never
fetches helper tools separately. All runtime files are covered by the installation
manifest and version switching/rollback. Install the complete native archive;
copying only `bin/kyler` is insufficient.

macOS/Linux and x86_64/ARM64 builds are supported. Linux still requires the host's
glibc ABI, kernel namespace capabilities and compatible security policy. Those
cannot be bundled as user-space executables. Both offline and outbound native
sandbox execution are mandatory checks before activation. No host security setting
is weakened and there is no unrestricted-execution fallback.

The Shell bootstrap needs basic shell/checksum utilities and curl for remote
downloads. The CLI also uses curl for HTTPS transfer and parses archives internally;
installation with an adjacent installer no longer requires a host tar executable.
For a standalone offline archive without that executable, the bootstrap can use tar
to read only its CLI and checksum manifest; the CLI verifies and extracts the
complete package. These bootstrap utilities are separate from Kyler's private
runtime. Node.js is included; Rust is only needed when building from source.

### Native sandbox self-check

After verification, before replacing the old installation or stopping services, the
new binary runs `kyler doctor` as the installing user. It uses temporary fixtures and
a disposable local TCP listener, without reading application configuration, contacting
a model or using an API key. Both offline and outbound sandbox policies must launch
successfully and enforce workspace writes, protected/denied/outside paths and denial
of host loopback access. A child shell and a real `grep_files` search over a
disposable fixture are also executed inside each sandbox. This verifies ripgrep discovery and access through
the same runtime PATH and read-only executable roots used by workspace tools. Worker
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

Bundles must provide both `install` and `doctor`; use a newly built complete
bundle. The new bootstrap does not fall back to legacy Shell installation logic. Updating only `install.sh` does not update an existing release binary.

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

An already unpacked bundle can be installed without the shell bootstrap:

```sh
./path/to/bundle/bin/kyler install --bundle ./path/to/bundle
```

The CLI accepts unpacked directories and `.tar.gz` archives, or downloads a release:

```sh
kyler install --bundle ./kyler-v0.5.0-aarch64-apple-darwin.tar.gz
kyler install --version 0.5.0
```

These paths share the same transaction and UI. Archive links, special files,
duplicate entries and paths outside the extraction directory are rejected before
any bundled program runs.

Archive names follow `kyler-v<version>-<target>.tar.gz`. The target identifies the
operating system and architecture. [Release management](releasing.md) defines packaging
requirements and artifact validation.

## Configuration stages

The final interactive installation step opens the configuration wizard. Run
`kyler configure` to open it again later. Each
required input is validated before proceeding.

### Model configuration

| Input | Requirement or default |
| --- | --- |
| Provider | OpenAI, SiliconFlow, or Custom（OpenAI Completion） |
| API URL | Custom（OpenAI Completion） only; official providers use fixed URLs |
| API key | Required; input is hidden |
| Model | Searchable keyboard selection, with manual entry and retry/cache actions |

Provider choices are **OpenAI**, **SiliconFlow**, and **Custom（OpenAI Completion）**.
OpenAI and SiliconFlow use their official API URLs without an address prompt. Only
Custom（OpenAI Completion） asks for a base URL; use it for local servers and compatible gateways.
Existing custom endpoints default to Custom（OpenAI Completion）.

Setup is an inline terminal wizard built with Rust `cliclack`. It keeps normal
terminal history, uses arrow-key selection, and filters lists as you type. Completed
questions collapse to answers beneath their labels; API keys are never echoed. Enter
continues, explicit **Back** items revisit selection steps, and Esc or Ctrl+C cancels.
It does not switch to a full-screen alternate terminal buffer.

Setup fetches model metadata, never a generation request. Loading has a progress
indicator and supports Esc/Ctrl+C. If discovery fails or times out, choose **Retry
discovery**, **Use cached models**, or **Enter model ID manually**. Clear the search to see these actions if no model
matches. Back-navigation keeps draft
values and avoids refetching an unchanged connection's directory.

For multiple configured providers, first select the connection to configure; other
profiles remain intact. Before saving, review the selected model, address, capacity,
and access scope. For a complete installed bundle, choose **Save and start** or **Save without starting**,
edit a previous step, or cancel. Source builds offer **Save configuration**.
No configuration is replaced until this final choice. Unattended setup retains
`--non-interactive` and does not open terminal widgets.

The normal wizard does not ask for a context window. Known capacity follows provider
capability rules; unknown capacity displays a clearly marked fallback budget. Existing
explicit window overrides are retained. Use `kyler configure --advanced` to supply a
manual token count (`128k` means 128000) or enter `auto` to restore automatic sizing.
The wizard validates output, safety, and summary reservations. Thinking defaults follow
the selected model; setup does not add a separate reasoning-level question.
Ctrl+C or **Cancel setup** exits without replacing the existing installation/configuration.

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

The installer uses a compact, left-aligned layout without decorative blank lines,
boxed headings or alignment padding. It inherits the terminal background and normal
text colors. Active controls use the terminal's cyan palette, success green,
attention yellow and errors red; selected options are bold without a background
fill. `NO_COLOR`, `TERM=dumb` and redirected output suppress color. Active steps
show a spinner; completed steps occupy one line. Configuration inputs keep secrets
hidden, including after validation errors. Model choices use unpadded numeric
indexes without scroll-arrow markers; search and arrow-key navigation remain
available. Configuration can start services and
display the Web address and login token together, followed by the primary
interface name/address. A single blank line separates these connection details
from the completed steps; no blank header precedes installation. Network
mode uses the detected interface address in the Web URL; local-only mode retains
the loopback URL and labels its access scope. Ambiguous routing produces an
unavailable message instead of selecting an arbitrary interface. Inspect services
and retrieve the token again with
`kyler service status` and `kyler service token`. The default browser address is
**http://127.0.0.1:30002**.

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
previous installation, configuration, shell startup content, and service state. When cleanup cannot be
verified, installation reports the failure for inspection. Old version directories remain available during activation for rollback and are
removed after successful activation and service recovery. A local installation lock prevents simultaneous activations.
Cancellation during activation also invokes recovery; cancellation during setup
does not publish the candidate configuration. [Troubleshooting](troubleshooting.md) covers service diagnostics
and recovery.

## Installation options

Run `sh install.sh` to install the latest release and configure it in the final step.
The installer automatically configures PATH for new terminals.

| Option | Behavior |
| --- | --- |
| `--bundle PATH` | Install a complete local archive or bundle directory |
| `--version VERSION` | Select a published release version |
| `-h`, `--help` | Display installation help |

Advanced options support automation and maintenance:

| Option | Behavior |
| --- | --- |
| `--configure` | Open configuration during installation |
| `--start` | Start services explicitly after activation |
| `--no-start` | Install without starting services or opening automatic setup; existing managed services are stopped during upgrades |
| `--no-configure` | Skip the configuration wizard; create defaults only when configuration is absent |
| `--non-interactive` | Use saved configuration and process environment without prompts |

These advanced options are also available on `kyler install`. The CLI accepts
`--config` for a separate configuration path and rejects `--server` and `--json`.
Runtime checks are validation-only, including in non-interactive mode; missing
payloads must be repaired with a complete bundle.

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

### Installation diagnostics

Successful environment and sandbox checks show short status messages. Their detailed
output is retained in `~/.kyler/logs/install.log` with private file permissions. Failed
checks show the diagnostic needed to understand the failure. API-key input and the
interactive configuration transcript are not written to this log.
