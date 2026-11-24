#!/bin/sh
# Install a complete release bundle, then open configuration and launch services.
set -eu
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
bundle= version= configure=true start=true non_interactive=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --bundle|--version)
            flag=$1
            [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$flag requires a value."
            case "$flag" in --bundle) bundle=$2 ;; --version) version=$2 ;; esac
            shift 2 ;;
        --no-configure) configure=false; shift ;;
        --configure) configure=true; shift ;;
        --no-start) start=false; shift ;;
        --start) start=true; shift ;;
        --non-interactive) non_interactive=true; shift ;;
        -h|--help)
            printf '%s\n' 'Usage: sh install.sh [--version VERSION | --bundle ARCHIVE_OR_DIRECTORY] [--no-configure] [--no-start] [--non-interactive]'
            exit 0 ;;
        *) fail "Unknown argument: $1" ;;
    esac
done
[ -z "$bundle" ] || [ -z "$version" ] || fail '--bundle and --version cannot be combined.'
case "${HOME:-}" in /*) ;; *) fail 'HOME must be an absolute directory.' ;; esac
umask 077
scratch=$(mktemp -d "${TMPDIR:-/tmp}/kyler-install.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
checksum() {
    if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}';
    else sha256sum "$1" | awk '{print $1}'; fi
}
verify() {
    expected=$(awk -v name="$3" '$2==name || $2=="*"name {print $1}' "$2")
    [ "${#expected}" = 64 ] || fail "Missing or invalid checksum: $3"
    [ "$(checksum "$1")" = "$expected" ] || fail "Checksum mismatch: $3"
}
transfer() {
    curl --fail --location --show-error --silent --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 2 --connect-timeout 15 --max-time 600 "$@"
}
if [ -z "$bundle" ]; then
    case "$(uname -s)" in Darwin) target=apple-darwin ;; Linux) target=unknown-linux-gnu ;; *) fail 'Unsupported operating system.' ;; esac
    case "$(uname -m)" in arm64|aarch64) arch=aarch64 ;; x86_64|amd64) arch=x86_64 ;; *) fail 'Unsupported architecture.' ;; esac
    repository=${KYLER_REPOSITORY:-yeskyler/Kyler}
    case "$repository" in *[!a-zA-Z0-9_./-]*|*..*|/*|*/|*/*/*) fail 'Invalid release repository.' ;; esac
    mirror=https://www.kyler.work/releases
    if [ -z "$version" ]; then
        if transfer "$mirror/latest.txt" -o "$scratch/latest"; then
            version=$(cat "$scratch/latest")
        else
            effective=$(transfer -o /dev/null -w '%{url_effective}' "https://github.com/$repository/releases/latest")
            version=${effective##*/}
        fi
    fi
    printf '%s\n' "$version" | LC_ALL=C grep -Eq '^v?[0-9][0-9A-Za-z.+-]*\.[0-9A-Za-z.+-]+$' || fail 'Invalid release version.'
    case "$version" in v*) ;; *) version=v$version ;; esac
    asset=kyler-$version-$arch-$target.tar.gz
    downloaded=false
    for base in "$mirror/$version" "https://github.com/$repository/releases/download/$version"; do
        if transfer "$base/SHA256SUMS" -o "$scratch/SHA256SUMS" && transfer "$base/$asset" -o "$scratch/$asset"; then
            verify "$scratch/$asset" "$scratch/SHA256SUMS" "$asset"
            downloaded=true
            break
        fi
    done
    [ "$downloaded" = true ] || fail 'Cannot download release bundle.'
    bundle=$scratch/$asset
fi
if [ -f "$bundle" ]; then
    # Reject traversal paths and links before extracting an archive.
    tar -tzf "$bundle" > "$scratch/entries"
    LC_ALL=C grep -Eq '(^/|(^|/)\.\.(/|$))' "$scratch/entries" && fail 'Unsafe archive path.'
    tar -tvzf "$bundle" > "$scratch/types"
    LC_ALL=C grep -Eq '^[^d-]' "$scratch/types" && fail 'Archive links and special files are not supported.'
    mkdir "$scratch/unpacked"
    tar -xzf "$bundle" -C "$scratch/unpacked"
    set -- "$scratch/unpacked"/*
    [ "$#" = 1 ] && [ -d "$1" ] || fail 'Archive must contain one bundle directory.'
    bundle=$1
fi
[ -d "$bundle" ] || fail 'Bundle not found.'
# Check every installed file before executing anything from the bundle.
find "$bundle" ! -type d ! -type f > "$scratch/special"
[ ! -s "$scratch/special" ] || fail 'Bundle links and special files are not supported.'
[ -f "$bundle/SHA256SUMS" ] || fail 'Bundle checksums are missing.'
while IFS= read -r entry; do
    hash=${entry%%  *}
    name=${entry#*  }
    case "$name" in ''|/*|../*|*/../*|*/..|..) fail 'Unsafe checksum path.' ;; esac
    [ "$hash" != "$entry" ] || fail 'Invalid checksum entry.'
    verify "$bundle/$name" "$bundle/SHA256SUMS" "$name"
done < "$bundle/SHA256SUMS"
(cd "$bundle" && find . -type f ! -path './SHA256SUMS' | sed 's|^./||' | LC_ALL=C sort) > "$scratch/files"
sed 's/^[^ ]*  //' "$bundle/SHA256SUMS" | LC_ALL=C sort > "$scratch/manifest"
cmp -s "$scratch/files" "$scratch/manifest" || fail 'Bundle manifest must describe every file exactly once.'
for required in bin/kyler runtime/node runtime/tools/rg web/server.js; do
    verify "$bundle/$required" "$bundle/SHA256SUMS" "$required"
done
root=$HOME/.local/share/kyler
mkdir -p "$root/versions" "$HOME/.local/bin"
staged=$(mktemp -d "$root/versions/release.XXXXXX")
cp -R "$bundle/." "$staged/"
ln -s "$staged" "$root/.current-$$"
[ ! -e "$root/current" ] || [ -L "$root/current" ] || fail 'Current installation path is not a symbolic link.'
rm -f "$root/current"
mv "$root/.current-$$" "$root/current"
ln -sf "$root/current/bin/kyler" "$HOME/.local/bin/kyler"
export PATH="$HOME/.local/bin:$PATH"
case "${SHELL:-}" in */zsh) profile=$HOME/.zshrc ;; *) profile=$HOME/.profile ;; esac
path_line='export PATH="$HOME/.local/bin:$PATH"'
if ! [ -f "$profile" ] || ! grep -Fxq "$path_line" "$profile"; then
    printf '\n# Kyler\n%s\n' "$path_line" >> "$profile"
fi
printf 'Kyler installed.\n'
if [ "$configure" = true ]; then
    if [ "$non_interactive" = true ]; then
        KYLER_CONFIGURE_NO_START=1 kyler configure --non-interactive --web --addresses
    elif [ -t 0 ]; then
        KYLER_CONFIGURE_NO_START=1 kyler configure --web --addresses
    elif [ -r /dev/tty ]; then
        KYLER_CONFIGURE_NO_START=1 kyler configure --web --addresses </dev/tty
    else
        fail 'Run kyler configure in a terminal to finish setup.'
    fi
fi
if [ "$start" = true ]; then
    kyler service start --restart
fi
