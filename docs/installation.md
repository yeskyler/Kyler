# Installation

Complete release bundles contain Kyler, the Web application, Node.js and private
runtime tools. No Rust or npm installation is required.

```sh
curl -fsSL https://www.kyler.work/install.sh -o install.sh
sh install.sh
```

The shell script downloads the release archive and SHA256SUMS, verifies the archive,
extracts it, verifies its files and installs it. It then runs `kyler configure
--web --addresses` and, after successful configuration, `kyler service start --restart`.
There is no separate installer executable or `kyler install` command. Installation
performs no environment diagnostics; run `kyler doctor` manually when needed.
If configuration is cancelled, the program stays installed; configuration and
service startup can be retried manually.

Downloads use the installation server first, with GitHub Releases as a fallback.
Checksum failures stop installation. Use `--version VERSION` to select a release.

## Local installation

```sh
sh scripts/install.sh --bundle /path/to/bundle.tar.gz
sh scripts/install.sh --bundle /path/to/unpacked-bundle
```

Local bundles require their internal SHA256SUMS. Only regular files and directories
are accepted. Obtain local bundles from a trusted source.

## Options

- `--version VERSION`: download a specific release.
- `--bundle PATH`: install a local archive or directory.
- `--no-configure`: skip configuration.
- `--no-start`: skip service startup.
- `--non-interactive`: configure using the CLI's unattended configuration mode.

For unattended file installation only, use `--no-configure --no-start`.

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

### Channels

Choose **Skip for now** to complete setup without a channel, or **Configure
channels** to connect one or more platforms. Setup saves the required platform
fields in the private configuration. To add or edit a channel later, run
`kyler configure` again and open **Channels**.

In Kyler Web, **Channels** lists every platform. Open a platform to view its
setup requirements or pair a chat with a workspace. See [Channels](channels.md)
for provider-specific setup.

### Service status and management

The configuration wizard uses a compact, left-aligned layout without decorative blank lines,
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
| Open the local terminal interface | `kyler` (or `kyler tui`) |

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

## Paths and updates

Versions are installed under `~/.local/share/kyler/versions/`, selected through
`~/.local/share/kyler/current`. The command is linked at `~/.local/bin/kyler`.
The script adds that directory to `.zshrc` for Zsh or `.profile` otherwise.
Configuration remains at `~/.kyler/config.toml`; installation preserves it.
Run the script again to upgrade. Previous version directories remain available,
but the script does not automatically roll back a failed configuration or startup.

```sh
kyler configure
kyler service start --restart
kyler service status
kyler logs
kyler doctor
```
