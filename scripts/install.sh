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
cleanup() { rm -rf "$scratch"; }
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
    for required in bin/kyler runtime/node web/server.js runtime/tools/rg; do
        awk -v file="$required" '$2==file {found=1} END {exit !found}' "$1/SHA256SUMS" || fail "Missing checksum for $required"
    done
    if [ "$host_os" = Linux ]; then
        for tool in bwrap slirp4netns ip unshare nsenter iptables ip6tables iptables-restore ip6tables-restore sh sleep; do
            awk -v file="runtime/sandbox/bin/$tool" '$2==file {found=1} END {exit !found}' "$1/SHA256SUMS" || fail "Missing bundled sandbox tool: $tool"
        done
    fi
    # Never let checksum entries address files outside the verified bundle.
    awk 'length($1)!=64 || $1 ~ /[^a-fA-F0-9]/ {bad=1} {name=substr($0,67); sub(/^\*/,"",name); if(name ~ /^\// || name ~ /(^|\/)\.\.($|\/)/) bad=1} END {exit bad}' "$1/SHA256SUMS" || fail 'Unsafe bundle checksum entry.'
    # Release bundles contain regular files and directories only.
    [ -z "$(find "$1" -type l -print -quit)" ] || fail 'Bundle symlinks are not supported.'
    if command -v shasum >/dev/null 2>&1; then (cd "$1" && shasum -a 256 -c SHA256SUMS >/dev/null) || fail 'Bundle checksum mismatch.';
    else (cd "$1" && sha256sum -c SHA256SUMS >/dev/null) || fail 'Bundle checksum mismatch.'; fi
}
step() { printf '\n[%s/2] %s\n' "$1" "$2"; }
check_system() {
    host_os=$(uname -s)
    case "$host_os" in Darwin) os=apple-darwin ;; Linux) os=unknown-linux-gnu ;; *) fail 'Kyler release bundles currently support macOS and Linux only.' ;; esac
    case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; x86_64|amd64) arch=x86_64 ;; *) fail 'Unsupported architecture; use an x86_64 or ARM64 host.' ;; esac
    printf 'Platform: %s %s\n' "$host_os" "$arch"
    for utility in tar awk sed find mktemp; do
        command -v "$utility" >/dev/null 2>&1 || fail "Required bootstrap command is missing: $utility"
    done
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
step 1 'Identify release platform'
check_system
step 2 'Download and verify bundle'
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
# The verified release CLI owns prerequisites, sandbox validation, configuration,
# activation and recovery. Keep this process alive until it exits so temporary
# extraction files are cleaned up even when setup is cancelled.
unset KYLER_INTERNAL_TOOL_WORKER KYLER_STARTUP_TRACE
[ -x "$local_binary" ] || fail 'Bundle binary is not executable.'
set -- --config "$HOME/.kyler/config.toml" install --bundle "$bundle"
[ "$start_server" = true ] || set -- "$@" --no-start
[ "$configure" = true ] || set -- "$@" --no-configure
[ "$non_interactive" = false ] || set -- "$@" --non-interactive
code=0
if [ "$non_interactive" = false ] && [ ! -t 0 ] && [ -r /dev/tty ] && ( : </dev/tty ) 2>/dev/null; then
    "$local_binary" "$@" </dev/tty || code=$?
else
    "$local_binary" "$@" || code=$?
fi
exit "$code"
