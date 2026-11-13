#!/bin/sh
# Install a complete release bundle for the current platform.
set -eu
# Namespace/firewall tools are commonly installed in sbin on Linux.
PATH=${PATH:-/usr/bin:/bin}:/usr/sbin:/sbin
# Include existing Homebrew installations without invoking shell startup files.
for dependency_bin in /opt/homebrew/bin /usr/local/bin; do
    [ ! -d "$dependency_bin" ] || PATH="$PATH:$dependency_bin"
done
export PATH
usage() {
    cat <<'HELP'
Usage: sh install.sh [OPTIONS]
  --bundle PATH       Install a complete local bundle directory or .tar.gz.
  --version VERSION   Download a specific release (default: latest).
  -h, --help          Show help.

Downloads: https://www.kyler.work/releases first, GitHub Releases as fallback.
Local installation: sh install.sh --bundle /path/to/kyler-platform.tar.gz

Complete bundles include the backend, Web UI and Node.js runtime.
Installation automatically adds ~/.local/bin to PATH for new terminals.
Defaults: Backend 127.0.0.1:30001; Web 127.0.0.1:30002.
Setup offers local-only or local-network access. Web access uses a generated token.
Existing values are kept with Enter. API keys are hidden.
Sandbox dependencies and real execution are checked before activation.
HELP
}
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
local_binary= bundle= requested_version= start_server=true configure=true non_interactive=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --bundle|--version)
            flag=$1; [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$flag requires a value."
            case "$2" in --*) fail "$flag requires a value." ;; esac
            case "$flag" in
                --bundle) [ -z "$bundle" ] || fail '--bundle specified twice.'; bundle=$2 ;;
                --version) requested_version=$2 ;;
            esac
            shift 2 ;;
        --no-start) start_server=false; shift ;;
        --no-configure) configure=false; shift ;;
        --non-interactive) non_interactive=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "Unknown argument: $1. Use --help." ;;
    esac
done
case "${HOME:-}" in /*) ;; *) fail 'HOME must be an absolute directory.' ;; esac
umask 077
scratch=$(mktemp -d "${TMPDIR:-/tmp}/kyler-install.XXXXXX") || fail 'Cannot create temporary directory.'
binary_stage= config_stage= config_backup= version_stage=
old_entry= old_current= old_binary= old_running=false
cleanup() {
    [ -z "$binary_stage" ] || rm -f "$binary_stage"
    [ -z "$config_stage" ] || rm -f "$config_stage"
    [ -z "$config_backup" ] || rm -f "$config_backup"
    [ -z "$version_stage" ] || rm -rf "$version_stage"
    rm -rf "$scratch"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
checksum() {
    if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}';
    elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
    else fail 'Install shasum or sha256sum first.'; fi
}
check_manifest() {
    [ -f "$1/SHA256SUMS" ] || fail 'Bundle SHA256SUMS is missing.'
    for required in bin/kyler runtime/node web/server.js; do
        awk -v file="$required" '$2==file {found=1} END {exit !found}' "$1/SHA256SUMS" || fail "Missing checksum for $required"
    done
    # Release bundles contain regular files and directories only.
    [ -z "$(find "$1" -type l -print -quit)" ] || fail 'Bundle symlinks are not supported.'
    if command -v shasum >/dev/null 2>&1; then (cd "$1" && shasum -a 256 -c SHA256SUMS >/dev/null) || fail 'Bundle checksum mismatch.';
    else (cd "$1" && sha256sum -c SHA256SUMS >/dev/null) || fail 'Bundle checksum mismatch.'; fi
}
step() { printf '\n[%s/6] %s\n' "$1" "$2"; }
check_system() {
    host_os=$(uname -s)
    case "$host_os" in Darwin) os=apple-darwin ;; Linux) os=unknown-linux-gnu ;; *) fail 'Kyler release bundles currently support macOS and Linux only.' ;; esac
    case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; x86_64|amd64) arch=x86_64 ;; *) fail 'Unsupported architecture; use an x86_64 or ARM64 host.' ;; esac
    printf 'Platform: %s %s\n' "$host_os" "$arch"
    if [ "$host_os" = Linux ]; then
        # Parse display metadata only; never execute os-release as shell code.
        if [ -r /etc/os-release ]; then
            distribution=$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | tr -d '\042\047' | head -n 1)
            printf 'Distribution: %s\n' "${distribution:-unknown}"
        fi
        libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
        case "$libc" in glibc\ *) printf 'Runtime: %s (bundle execution will verify compatibility)\n' "$libc" ;;
            *) fail 'Current Linux bundles require glibc. musl/Alpine and unidentified runtimes need a separately compatible build; no system libraries were changed.' ;; esac
    fi
    for utility in tar awk sed find mktemp; do
        command -v "$utility" >/dev/null 2>&1 || fail "Required command is missing: $utility"
    done
}
prepare_runtime_dependencies() {
    if [ "$(uname -s)" = Darwin ]; then
        printf 'Sandbox: system Seatbelt (actual execution checked in step 4).\n'
        packages=
        rg --version >/dev/null 2>&1 || packages=ripgrep
        command -v curl >/dev/null 2>&1 || packages="$packages curl"
        if [ -n "$packages" ]; then
            command -v brew >/dev/null 2>&1 || fail "Missing dependencies: $packages. Install Homebrew using https://brew.sh, then run: brew install $packages. Rerun this installer afterward."
            printf 'Installing missing dependencies. Command: brew install%s\n' " $packages"
            brew install $packages || fail "Dependency installation failed. Retry: brew install $packages"
        else
            printf 'Reusing installed ripgrep and download tools.\n'
        fi
        rg --version || fail 'ripgrep is still unavailable; check your Homebrew installation and PATH.'
        command -v curl >/dev/null 2>&1 || fail 'curl is still unavailable.'
        command -v shasum >/dev/null 2>&1 || command -v sha256sum >/dev/null 2>&1 || fail 'SHA256 checksum tools are missing.'
        return 0
    fi
    printf 'Checking Linux sandbox dependencies (Bubblewrap >= 0.5.0 and isolated networking)...\n'
    bubblewrap_ready() {
        command -v bwrap >/dev/null 2>&1 || return 1
        bwrap_version=$(bwrap --version) || return 1
        printf '%s\n' "$bwrap_version"
        printf '%s\n' "$bwrap_version" | awk 'NF==2 && $1=="bubblewrap" && $2 ~ /^[0-9]+\.[0-9]+\.[0-9]+$/ {split($2,v,"."); ok=(v[1]+0 > 0 || v[2]+0 >= 5)} END {exit !ok}'
    }
    dependencies_ready() {
        missing=
        packages=
        bubblewrap_ready || { missing=bwrap; packages=bubblewrap; }
        rg --version >/dev/null 2>&1 || { missing="$missing rg"; packages="$packages ripgrep"; }
        command -v curl >/dev/null 2>&1 || { missing="$missing curl"; packages="$packages curl ca-certificates"; }
        if ! command -v shasum >/dev/null 2>&1 && ! command -v sha256sum >/dev/null 2>&1; then
            missing="$missing sha256sum"; packages="$packages coreutils"
        fi
        for tool in slirp4netns ip unshare nsenter iptables ip6tables iptables-restore ip6tables-restore sh sleep; do
            if ! command -v "$tool" >/dev/null 2>&1; then
                missing="$missing $tool"
                case "$tool" in
                    slirp4netns) packages="$packages slirp4netns" ;;
                    ip) packages="$packages $ip_package" ;;
                    unshare|nsenter) case " $packages " in *' util-linux '*) ;; *) packages="$packages util-linux" ;; esac ;;
                    iptables*) case " $packages " in *' iptables '*) ;; *) packages="$packages iptables" ;; esac ;;
                    sh|sleep) fail "Required base system command missing: $tool" ;;
                esac
            fi
        done
        [ -z "$missing" ]
    }
    manager=
    for candidate in apt-get dnf yum pacman zypper; do
        if command -v "$candidate" >/dev/null 2>&1; then manager=$candidate; break; fi
    done
    case "$manager" in dnf|yum) ip_package=iproute ;; *) ip_package=iproute2 ;; esac
    if dependencies_ready; then
        rg --version
        printf 'Reusing installed Bubblewrap, networking tools, ripgrep and download tools.\n'
        return 0
    fi
    printf 'Missing or outdated: %s\n' "$missing"
    case "$manager" in
        apt-get) instruction="apt-get update && apt-get install -y $packages" ;;
        dnf|yum) instruction="$manager install -y $packages" ;;
        pacman) instruction="pacman -S --needed --noconfirm $packages" ;;
        zypper) instruction="zypper --non-interactive install $packages" ;;
        *) fail "No supported package manager found. Ask an administrator to install these missing dependencies: $missing. Then rerun the installer." ;;
    esac
    if [ "$(id -u)" = 0 ]; then elevated=; else
        command -v sudo >/dev/null 2>&1 || fail "Administrator access is required. Ask an administrator to run: $instruction"
        elevated=sudo
    fi
    printf 'Preparing dependencies using %s. Administrator access may be required.\nManual command (as administrator): %s\n' "$manager" "$instruction"
    # Package names are fixed above; no repository metadata is executed as a command.
    case "$manager" in
        apt-get)
            $elevated apt-get update && $elevated apt-get install -y $packages || fail "Dependency installation failed. Run as administrator: $instruction" ;;
        dnf|yum) $elevated "$manager" install -y $packages || fail "Dependency installation failed. Run as administrator: $instruction" ;;
        pacman) $elevated pacman -S --needed --noconfirm $packages || fail "Dependency installation failed. Run as administrator: $instruction" ;;
        zypper) $elevated zypper --non-interactive install $packages || fail "Dependency installation failed. Run as administrator: $instruction" ;;
    esac
    dependencies_ready || fail "Dependencies remain unavailable: $missing. The system repository may be too old or commands may be missing from PATH. No third-party repositories were added."
    rg --version
    printf 'Dependency commands ready; native sandbox execution will be checked after unpacking.\n'
}
transfer() {
    curl --fail --location --show-error --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --connect-timeout 15 --speed-limit 1024 --speed-time 60 --retry 2 --retry-delay 2 "$@"
}
valid_version() {
    printf '%s\n' "$1" | LC_ALL=C grep -Eq '^v?[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$'
}
manual_help() {
    printf '\nManual installation:\n' >&2
    printf '  curl -fL %s/%s -o %s\n' "$github_release" "$asset" "$asset" >&2
    printf '  curl -fL %s/SHA256SUMS -o SHA256SUMS\n' "$github_release" >&2
    if command -v shasum >/dev/null 2>&1; then
        printf '  grep "  %s$" SHA256SUMS | shasum -a 256 -c -\n' "$asset" >&2
    else
        printf '  grep "  %s$" SHA256SUMS | sha256sum -c -\n' "$asset" >&2
    fi
    printf '  sh install.sh --bundle ./%s\n' "$asset" >&2
    printf 'Verify the archive against SHA256SUMS before installing.\n' >&2
}
printf '\nKyler installer\n\nInstall location: %s/.local/share/kyler\nConfiguration: %s/.kyler/config.toml\n' "$HOME" "$HOME"
printf 'Existing configuration can be kept with Enter. API keys are hidden.\n'
printf 'Already downloaded a bundle? Run: sh install.sh --bundle /path/to/kyler-platform.tar.gz\n'
step 1 'Check system compatibility'
check_system
step 2 'Prepare runtime dependencies'
prepare_runtime_dependencies
step 3 'Download and verify bundle'
if [ -z "$bundle" ]; then
    command -v curl >/dev/null 2>&1 || fail 'curl is required.'
    command -v shasum >/dev/null 2>&1 || command -v sha256sum >/dev/null 2>&1 || fail 'Install shasum or sha256sum first.'
    repository=${KYLER_REPOSITORY:-yeskyler/Kyler}
    case "$repository" in *[!a-zA-Z0-9_./-]*|*..*|/*|*/|*/*/*) fail 'Invalid release repository.' ;; esac
    mirror=https://www.kyler.work/releases
    printf 'Download order: %s -> GitHub Releases\n' "$mirror"
    if [ -z "$requested_version" ]; then
        printf 'Checking latest version on Kyler download server...\n'
        if transfer --silent --max-time 45 "$mirror/latest.txt" -o "$scratch/latest" &&
            requested_version=$(cat "$scratch/latest") && valid_version "$requested_version"; then
            :
        else
            printf 'Server version lookup failed; checking GitHub Releases...\n'
            effective=$(transfer --silent --max-time 45 -o /dev/null -w '%{url_effective}' "https://github.com/$repository/releases/latest") || fail 'Cannot determine latest version. Retry with --version VERSION.'
            requested_version=${effective##*/}
        fi
    fi
    valid_version "$requested_version" || fail 'Invalid release version.'
    case "$requested_version" in v*) ;; *) requested_version=v$requested_version ;; esac
    asset=kyler-$requested_version-$arch-$os.tar.gz
    github_release=https://github.com/$repository/releases/download/$requested_version
    printf 'Version: %s\nPlatform: %s-%s\nFile: %s\n' "$requested_version" "$arch" "$os" "$asset"
    cache_dir=${XDG_CACHE_HOME:-$HOME/.cache}/kyler/downloads/$requested_version
    mkdir -p "$cache_dir"
    printf 'Download cache: %s\n' "$cache_dir"
    downloaded=false
    for source in "$mirror/$requested_version" "$github_release"; do
        printf '\nSource: %s\nFetching release checksums...\n' "$source"
        rm -f "$scratch/release-checksums"
        if transfer "$source/SHA256SUMS" -o "$scratch/release-checksums"; then
            expected=$(awk -v name="$asset" '$2==name || $2=="*"name {print $1}' "$scratch/release-checksums")
            [ "${#expected}" = 64 ] && printf '%s\n' "$expected" | LC_ALL=C grep -Eq '^[a-fA-F0-9]{64}$' || fail 'Release checksum entry is missing or invalid.'
            for candidate in "$cache_dir/$asset" "$PWD/$asset"; do
                if [ -f "$candidate" ]; then
                    printf 'Checking existing bundle: %s\n' "$candidate"
                    if [ "$expected" = "$(checksum "$candidate")" ]; then
                        printf 'Reusing verified bundle; download skipped.\n'
                        bundle=$candidate
                        downloaded=true
                        break
                    fi
                    printf 'Existing bundle checksum differs; a fresh download is required.\n'
                fi
            done
            if [ "$downloaded" != true ]; then
                printf 'Downloading Kyler %s (size, speed and remaining time below)...\n' "$requested_version"
                if transfer "$source/$asset" -o "$scratch/$asset"; then
                    [ "$expected" = "$(checksum "$scratch/$asset")" ] || fail 'Release checksum mismatch. Installation stopped.'
                    cache_stage=$(mktemp "$cache_dir/.download-XXXXXX")
                    if ! cp "$scratch/$asset" "$cache_stage" || ! mv -f "$cache_stage" "$cache_dir/$asset"; then
                        rm -f "$cache_stage"
                        fail 'Cannot save verified bundle in download cache.'
                    fi
                    bundle=$cache_dir/$asset
                    downloaded=true
                    printf 'Verified bundle saved in cache.\n'
                fi
            fi
            [ "$downloaded" != true ] || break
        fi
        if [ "$source" != "$github_release" ]; then
            printf 'Server download failed; switching to GitHub for the same version %s.\n' "$requested_version"
        fi
    done
    if [ "$downloaded" != true ]; then
        manual_help
        fail 'Both download sources failed. Existing installation was kept.'
    fi
    expected=$(awk -v name="$asset" '$2==name || $2=="*"name {print $1}' "$scratch/release-checksums")
    [ -n "$expected" ] && [ "$expected" = "$(checksum "$bundle")" ] || fail 'Release checksum mismatch. Installation stopped.'
    printf 'Release SHA256 checksum verified.\n'
else
    printf 'Use local bundle (download skipped).\n'
    printf 'Bundle: %s\n' "$bundle"
fi
if [ -n "$bundle" ]; then
    case "$bundle" in /*) ;; *) bundle=$PWD/$bundle ;; esac
    if [ ! -d "$bundle" ]; then
        [ -f "$bundle" ] || fail 'Bundle not found.'
        tar -tzf "$bundle" > "$scratch/members" || fail 'Invalid bundle archive.'
        awk '/^\// || /(^|\/)\.\.($|\/)/ {bad=1} END {exit bad}' "$scratch/members" || fail 'Archive contains unsafe paths.'
        tar -tvzf "$bundle" > "$scratch/types" || fail 'Cannot inspect archive.'
        awk 'substr($0,1,1)!="-" && substr($0,1,1)!="d" {bad=1} END {exit bad}' "$scratch/types" || fail 'Archive links and special files are not supported.'
        mkdir "$scratch/unpack"
        tar -xzf "$bundle" -C "$scratch/unpack" || fail 'Cannot unpack release.'
        roots=$(find "$scratch/unpack" -mindepth 1 -maxdepth 1 -type d)
        [ "$(printf '%s\n' "$roots" | wc -l | tr -d ' ')" = 1 ] && [ -n "$roots" ] || fail 'Expected one bundle root directory.'
        bundle=$roots
    fi
    check_manifest "$bundle"
    [ -f "$bundle/bin/kyler" ] && [ -f "$bundle/runtime/node" ] && [ -f "$bundle/web/server.js" ] || fail 'Bundle must include bin/kyler, runtime/node and web/server.js.'
    printf 'Bundle files verified.\n'
    local_binary=$bundle/bin/kyler
fi
case "$local_binary" in /*) ;; *) local_binary=$PWD/$local_binary ;; esac
[ -f "$local_binary" ] && [ -r "$local_binary" ] || fail "Binary not found or unreadable: $local_binary"
step 4 'Check application and sandbox'
install_dir=$HOME/.local/bin
installed_binary=$install_dir/kyler
config_dir=$HOME/.kyler
config_path=$config_dir/config.toml
data_dir=$HOME/.local/share/kyler
[ ! -d "$installed_binary" ] || fail "$installed_binary is a directory."
[ ! -d "$config_path" ] || fail "$config_path is a directory."
binary_stage=$(mktemp "$scratch/kyler-check.XXXXXX")
cp "$local_binary" "$binary_stage"; chmod 755 "$binary_stage"
unset KYLER_INTERNAL_TOOL_WORKER KYLER_STARTUP_TRACE
version=$("$binary_stage" --version) || fail 'The new binary cannot run on this machine; installed version was kept.'
case "$version" in 'kyler '[0-9]*) ;; *) fail 'File did not report a kyler version.' ;; esac
case "${version#kyler }" in *[!a-zA-Z0-9.+-]*) fail 'Invalid binary version.' ;; esac
"$bundle/runtime/node" --version || fail 'Bundled Node.js cannot run on this machine; check its architecture and runtime requirements. Previous installation was kept.'
printf 'Checking native sandbox with the new binary as the current user...\n'
"$binary_stage" doctor || fail 'Sandbox self-check failed. Previous installation and configuration were kept. Inspect the diagnostic above; check Linux user namespaces, AppArmor or container restrictions with your administrator.'
mkdir -p "$data_dir/versions"
[ ! -e "$data_dir/current" ] || [ -L "$data_dir/current" ] || fail 'current must be a version symlink.'
bundle_id=$(checksum "$bundle/SHA256SUMS")
version_dir=$data_dir/versions/v${version#kyler }-$(printf %.12s "$bundle_id")
if [ -d "$version_dir" ]; then check_manifest "$version_dir";
else
    version_stage=$(mktemp -d "$data_dir/versions/.install-XXXXXX")
    cp -R "$bundle/." "$version_stage/"
    mv "$version_stage" "$version_dir"; version_stage=
fi
step 5 'Configure and install'
mkdir -p "$install_dir" "$config_dir"
if [ ! -e "$config_path" ]; then
    cat > "$config_path" <<'CONFIG'
[llm]
model = "deepseek-ai/DeepSeek-V4-Flash"
base_url = "https://api.siliconflow.cn/v1"
# Supply credentials during setup or through the process environment.
[llm.capabilities]
images = true
[sandbox]
mode = "workspace-write"
network_access = true
[workspace]
default_directory = "workspace"
[session]
enabled = true
directory = "sessions"
[context]
context_tokens = 128000
window_ratio = 0.6
trigger_ratio = 0.8
[server]
bind = "127.0.0.1:30001"
[client]
server_url = "http://127.0.0.1:30001"
CONFIG
    printf '\n[web]\nenabled = true\nbind = "127.0.0.1:30002"\n' >> "$config_path"
    chmod 600 "$config_path"
else
    printf 'Preserved %s\n' "$config_path"
fi
if [ "$configure" = true ]; then
    config_stage=$(mktemp "$config_dir/.config-install.XXXXXX")
    set -- --config "$config_path" configure --output "$config_stage" --addresses
    set -- "$@" --web
    [ "$non_interactive" = false ] || set -- "$@" --non-interactive
    code=0
    # A piped installer still reads interactive answers from the controlling terminal.
    if [ "$non_interactive" = false ] && [ ! -t 0 ] && [ -r /dev/tty ] && ( : </dev/tty ) 2>/dev/null; then
        "$binary_stage" "$@" </dev/tty || code=$?
    else "$binary_stage" "$@" || code=$?; fi
    [ "$code" -ne 130 ] || { printf 'Setup cancelled; installed binary was kept.\n' >&2; exit 130; }
    [ "$code" -eq 0 ] || fail 'Configuration failed; installed binary was kept.'
fi
config_backup=$(mktemp "$config_dir/.config-before-install.XXXXXX")
cp "$config_path" "$config_backup"
[ ! -L "$installed_binary" ] || old_entry=$(readlink "$installed_binary")
[ ! -L "$data_dir/current" ] || old_current=$(readlink "$data_dir/current")
if [ -f "$installed_binary" ] && [ -z "$old_entry" ]; then old_binary=$scratch/previous-kyler; cp "$installed_binary" "$old_binary"; chmod 755 "$old_binary"; fi
installed_service() (
    # Setup captures overrides in the saved configuration. Lifecycle operations use that file.
    unset KYLER_SERVER_BIND KYLER_SERVER_TOKEN KYLER_WEB_BIND KYLER_WEB_TOKEN KYLER_WEB_ENABLED KYLER_CLIENT_SERVER_URL KYLER_CLIENT_TOKEN KYLER_WEB_BUNDLE
    "$installed_binary" --config "$config_path" "$@"
)
if [ -x "$installed_binary" ] && { [ -f "$config_dir/server.json" ] || [ -f "$config_dir/web.json" ]; }; then
    state=$(installed_service --json service status) || fail 'Cannot verify existing services; installation was not activated.'
    case "$state" in *'"running":true'*) old_running=true ;; esac
    installed_service service stop || fail 'Cannot stop existing managed services.'
fi
atomic_link() {
    "$bundle/runtime/node" -e 'const fs=require("fs"); const [target,dest]=process.argv.slice(1); const stage=dest+".install-"+process.pid; try{fs.symlinkSync(target,stage);fs.renameSync(stage,dest)}finally{try{fs.unlinkSync(stage)}catch{}}' "$1" "$2"
}
rollback() {
    printf 'Startup failed; restoring the previous installation...\n' >&2
    installed_service service stop >/dev/null 2>&1 || { printf 'Cleanup is not confirmed. Inspect kyler service status and logs before retrying.\n' >&2; return 1; }
    if [ -n "$old_current" ]; then atomic_link "$old_current" "$data_dir/current"; else rm -f "$data_dir/current"; fi
    if [ -n "$old_entry" ]; then atomic_link "$old_entry" "$installed_binary";
    elif [ -n "$old_binary" ]; then restore_binary=$(mktemp "$install_dir/.kyler-restore.XXXXXX"); cp "$old_binary" "$restore_binary"; chmod 755 "$restore_binary"; mv -f "$restore_binary" "$installed_binary";
    else rm -f "$installed_binary"; fi
    # apply follows existing configuration symlinks and preserves private permissions.
    "$binary_stage" --config "$config_path" configure --apply "$config_backup" || return 1
    if [ "$old_running" = true ]; then installed_service service start || return 1; fi
    printf 'Previous installation restored.\n' >&2
}
if [ -n "$config_stage" ] && ! "$binary_stage" --config "$config_path" configure --apply "$config_stage"; then
    if [ "$old_running" = true ]; then installed_service service start || fail 'Configuration was preserved, but the previous services could not restart.'; fi
    fail 'Cannot publish configuration; the previous installation was preserved.'
fi
atomic_link "$version_dir" "$data_dir/current"
atomic_link "$data_dir/current/bin/kyler" "$installed_binary"
printf '\nInstalled %s\n' "$version"
case "${SHELL:-}" in */zsh) startup_file=$HOME/.zshrc ;; */bash) startup_file=$HOME/.bashrc ;; *) startup_file=$HOME/.profile ;; esac
[ ! -d "$startup_file" ] || fail "$startup_file is a directory."
path_line='export PATH="$HOME/.local/bin:$PATH"'
if ! { [ -f "$startup_file" ] && grep -F -x "$path_line" "$startup_file" >/dev/null 2>&1; }; then printf '\n# Kyler\n%s\n' "$path_line" >> "$startup_file"; fi
printf 'PATH ready for new terminals (%s).\n' "$startup_file"
if [ "$(command -v kyler 2>/dev/null || true)" != "$installed_binary" ]; then printf 'For this terminal, run:\n  export PATH="$HOME/.local/bin:$PATH"\n'; fi
step 6 'Start and check services'
if [ "$start_server" = true ]; then
    printf 'Starting services...\n'
    if ! installed_service --json service start --restart >/dev/null; then
        rollback || fail 'Automatic recovery needs attention; inspect service status and logs.'
        fail 'Services could not start. Check kyler service status, ~/.kyler/logs/server.log and ~/.kyler/logs/web.log.'
    fi
    printf '\nKyler · Service status and management\n\n'
    installed_service service status
    if web_token=$(installed_service service token 2>/dev/null); then
        printf '\nWeb token: %s\n' "$web_token"
        unset web_token
    else
        printf '\nWeb token unavailable. Run kyler configure --web.\n'
    fi
else printf '\nKyler · Service status and management\n\nServices not started.\n'; fi
printf '\nInstallation complete.\n'
printf '\nConfiguration: %s\n' "$config_path"
printf '\nService management:\n  Status:   kyler service status\n  Stop:     kyler service stop\n  Start:    kyler service start\n  Restart:  kyler service start --restart\n  Token:    kyler service token\n  Diagnose: kyler doctor\n  Logs:     kyler logs backend\n  Web logs: kyler logs web\n'
printf '\nRun in your project:\n  kyler tui\n'
