#!/bin/sh
# Obtain a verified CLI installer; all bundle handling and UI belongs to Rust.
set -eu
usage() {
    cat <<'HELP'
Usage: sh install.sh [OPTIONS]
  --bundle PATH       Install a local bundle directory or .tar.gz archive.
  --version VERSION   Download a specific release (default: latest).
  -h, --help          Show help.

The CLI owns download progress, bundle verification, installation and setup.
Complete bundles include the backend, Web UI, Node.js and private runtime tools.
HELP
}
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
bundle= version= start=auto configure=auto non_interactive=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --bundle|--version)
            flag=$1
            [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$flag requires a value."
            case "$2" in --*) fail "$flag requires a value." ;; esac
            case "$flag" in --bundle) bundle=$2 ;; --version) version=$2 ;; esac
            shift 2 ;;
        --start) start=true; shift ;;
        --no-start) start=false; shift ;;
        --configure) configure=true; shift ;;
        --no-configure) configure=false; shift ;;
        --non-interactive) non_interactive=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "Unknown argument: $1. Use --help." ;;
    esac
done
[ -z "$bundle" ] || [ -z "$version" ] || fail '--bundle and --version cannot be combined.'
case "${HOME:-}" in /*) ;; *) fail 'HOME must be an absolute directory.' ;; esac
case "$(uname -s)" in Darwin) target_os=apple-darwin ;; Linux) target_os=unknown-linux-gnu ;; *) fail 'Kyler release bundles currently support macOS and Linux only.' ;; esac
case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; x86_64|amd64) arch=x86_64 ;; *) fail 'Unsupported architecture; use an x86_64 or ARM64 host.' ;; esac
if [ "$target_os" = unknown-linux-gnu ] && command -v ldd >/dev/null 2>&1; then
    case "$(ldd --version 2>&1 || true)" in *musl*) fail 'Linux release bundles require glibc; musl hosts are not supported.' ;; esac
fi
umask 077
scratch=$(mktemp -d "${TMPDIR:-/tmp}/kyler-bootstrap.XXXXXX") || fail 'Cannot create temporary directory.'
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
verify_installer() {
    expected=$(awk -v name="$3" '$2==name || $2=="*"name {print $1}' "$2")
    [ "${#expected}" = 64 ] && printf '%s\n' "$expected" | LC_ALL=C grep -Eq '^[a-fA-F0-9]{64}$' || fail 'Installer checksum entry is missing or invalid.'
    [ "$(checksum "$1")" = "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" ] || fail 'Installer checksum mismatch.'
}
transfer() {
    curl --silent --fail --location --show-error --proto '=https' --proto-redir '=https' --tlsv1.2 --connect-timeout 15 --max-time 180 --retry 2 "$@"
}
valid_version() {
    printf '%s\n' "$1" | LC_ALL=C grep -Eq '^v?[0-9][0-9A-Za-z.+-]*\.[0-9A-Za-z.+-]+$'
}
# An explicit trusted installer is useful for source development and offline setups.
installer=${KYLER_INSTALLER_BINARY:-}
if [ -n "$installer" ]; then
    [ -x "$installer" ] || fail 'KYLER_INSTALLER_BINARY must name an executable CLI.'
fi
if [ -z "$installer" ] && [ -n "$bundle" ]; then
    case "$bundle" in /*) ;; *) bundle=$PWD/$bundle ;; esac
    if [ -d "$bundle" ]; then
        verify_installer "$bundle/bin/kyler" "$bundle/SHA256SUMS" bin/kyler
        cp "$bundle/bin/kyler" "$scratch/kyler-installer"
        installer=$scratch/kyler-installer
    elif [ -f "$bundle" ]; then
        base=${bundle##*/}
        asset=kyler-installer-${base#kyler-}
        asset=${asset%.tar.gz}
        candidate=${bundle%/*}/$asset
        if [ -f "$candidate" ] && [ -f "${bundle%/*}/SHA256SUMS" ]; then
            verify_installer "$candidate" "${bundle%/*}/SHA256SUMS" "$asset"
            cp "$candidate" "$scratch/kyler-installer"
            installer=$scratch/kyler-installer
        elif command -v tar >/dev/null 2>&1; then
            # Standalone offline archives can supply only the bootstrap executable.
            # The CLI still performs all complete archive extraction and verification.
            root=${base%.tar.gz}
            if tar -xOzf "$bundle" "$root/SHA256SUMS" >"$scratch/SHA256SUMS" 2>"$scratch/archive.log" &&
               tar -xOzf "$bundle" "$root/bin/kyler" >"$scratch/kyler-installer" 2>>"$scratch/archive.log"; then
                verify_installer "$scratch/kyler-installer" "$scratch/SHA256SUMS" bin/kyler
                installer=$scratch/kyler-installer
            fi
        fi
    else fail 'Bundle not found.'; fi
fi
if [ -z "$installer" ]; then
    command -v curl >/dev/null 2>&1 || fail 'curl is required to fetch the CLI installer.'
    if [ -t 2 ] && [ "${TERM:-}" != dumb ]; then printf 'Preparing installer...\r' >&2; fi
    repository=${KYLER_REPOSITORY:-yeskyler/Kyler}
    case "$repository" in *[!a-zA-Z0-9_./-]*|*..*|/*|*/|*/*/*) fail 'Invalid release repository.' ;; esac
    mirror=https://www.kyler.work/releases
    bootstrap_version=$version
    if [ -z "$bootstrap_version" ]; then
        if transfer "$mirror/latest.txt" -o "$scratch/latest" 2>"$scratch/download.log" && valid_version "$(cat "$scratch/latest")"; then
            bootstrap_version=$(cat "$scratch/latest")
        else
            effective=$(transfer -o /dev/null -w '%{url_effective}' "https://github.com/$repository/releases/latest" 2>>"$scratch/download.log") || fail 'Cannot determine installer version. Retry with --version VERSION.'
            bootstrap_version=${effective##*/}
        fi
    fi
    valid_version "$bootstrap_version" || fail 'Invalid release version.'
    case "$bootstrap_version" in v*) ;; *) bootstrap_version=v$bootstrap_version ;; esac
    asset=kyler-installer-$bootstrap_version-$arch-$target_os
    for base in "$mirror/$bootstrap_version" "https://github.com/$repository/releases/download/$bootstrap_version"; do
        if transfer "$base/SHA256SUMS" -o "$scratch/SHA256SUMS" 2>>"$scratch/download.log" && transfer "$base/$asset" -o "$scratch/kyler-installer" 2>>"$scratch/download.log"; then
            verify_installer "$scratch/kyler-installer" "$scratch/SHA256SUMS" "$asset"
            installer=$scratch/kyler-installer
            break
        fi
    done
    if [ -z "$installer" ]; then
        mkdir -p "$HOME/.kyler/logs"
        cp "$scratch/download.log" "$HOME/.kyler/logs/install-bootstrap.log"
        chmod 600 "$HOME/.kyler/logs/install-bootstrap.log"
        fail "Cannot download the CLI installer. Log: $HOME/.kyler/logs/install-bootstrap.log"
    fi
    # Pin the bundle to the exact release that supplied its installer.
    [ -n "$bundle" ] || version=$bootstrap_version
    if [ -t 2 ] && [ "${TERM:-}" != dumb ]; then printf '\r\033[2K' >&2; fi
fi
case "$installer" in "$scratch/"*) chmod 700 "$installer" ;; esac
set -- install
if [ -n "$bundle" ]; then set -- "$@" --bundle "$bundle";
elif [ -n "$version" ]; then set -- "$@" --version "$version"; fi
case "$start" in true) set -- "$@" --start ;; false) set -- "$@" --no-start ;; esac
case "$configure" in true) set -- "$@" --configure ;; false) set -- "$@" --no-configure ;; esac
[ "$non_interactive" = false ] || set -- "$@" --non-interactive
unset KYLER_INTERNAL_TOOL_WORKER KYLER_STARTUP_TRACE
code=0
if [ "$non_interactive" = false ] && [ ! -t 0 ] && [ -r /dev/tty ] && ( : </dev/tty ) 2>/dev/null; then
    "$installer" --config "$HOME/.kyler/config.toml" "$@" </dev/tty || code=$?
else
    "$installer" --config "$HOME/.kyler/config.toml" "$@" || code=$?
fi
exit "$code"
