"""Pinned standalone ripgrep for native release bundles."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request
import urllib.error

RG_VERSION = "15.2.0"
RG_DIGESTS = {
    "aarch64-apple-darwin": "3750b2e93f37e0c692657da574d7019a101c0084da05a790c83fd335bad973e4",
    "x86_64-apple-darwin": "af7825fcc69a2afc7a7aea55fc9af90e26421d8f20fe59df32e233c0b8a231c1",
    "aarch64-unknown-linux-musl": "800b1e7206afe799dfb5a6901f23147cfaabe0e52210538100f61e86e1740915",
    "x86_64-unknown-linux-musl": "33e15bcf1624b25cdd2a55813a47a2f95dbe126268203e76aa6a585d1e7b149c",
}

def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def fetch(url, destination):
    try:
        with urllib.request.urlopen(url, timeout=120) as response, Path(destination).open("wb") as output:
            shutil.copyfileobj(response, output)
    except urllib.error.URLError:
        if not shutil.which("curl"):
            raise
        subprocess.run(["curl", "--disable", "--fail", "--location", "--show-error",
                        "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2",
                        "--connect-timeout", "15", "--max-time", "120", "--retry", "2",
                        url, "-o", str(destination)], check=True)


def extract_regular_archive(archive, destination):
    with tarfile.open(archive, "r:gz") as source:
        for member in source.getmembers():
            path = Path(member.name)
            if path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
                raise ValueError("Runtime archive contains unsafe paths, links or special files")
        source.extractall(destination)


def check_ripgrep(binary, node_os):
    output = subprocess.check_output([str(binary), "--version"], text=True, timeout=10)
    if not output.startswith("ripgrep "):
        raise ValueError("Invalid ripgrep executable")
    if node_os == "darwin":
        libraries = subprocess.check_output(["otool", "-L", str(binary)], text=True, timeout=10)
        for line in libraries.splitlines()[1:]:
            path = line.strip().split(" (", 1)[0]
            if not path.startswith(("/usr/lib/", "/System/Library/")):
                raise ValueError(f"ripgrep must not depend on Homebrew or external libraries: {path}")
    else:
        libraries = subprocess.run(["ldd", str(binary)], text=True, capture_output=True, timeout=10)
        combined = libraries.stdout + libraries.stderr
        if libraries.returncode == 0 and "statically linked" not in combined:
            raise ValueError("Bundled Linux ripgrep must be a standalone static executable")
        if libraries.returncode != 0 and not any(text in combined for text in ("not a dynamic executable", "statically linked")):
            raise ValueError("Unable to verify static Linux ripgrep")
    return output.splitlines()[0]


def build_ripgrep(scratch, target=None):
    if not shutil.which("cargo"):
        raise ValueError("Prebuilt ripgrep download failed; install Cargo or provide --ripgrep-binary")
    root = scratch / "built-ripgrep"
    print(f"Building pinned ripgrep {RG_VERSION} from published Rust sources (no PCRE2 feature)", flush=True)
    command = ["cargo", "install", "ripgrep", "--version", "="+RG_VERSION, "--locked", "--root", str(root)]
    if target:
        if not shutil.which("rustup"):
            raise ValueError("Static Linux source builds need rustup's musl target or --ripgrep-binary")
        installed = subprocess.check_output(["rustup", "target", "list", "--installed"], text=True).splitlines()
        if target not in installed:
            subprocess.run(["rustup", "target", "add", target], check=True)
        command.extend(["--target", target])
    subprocess.run(command, check=True)
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    source = next((cargo_home / "registry/src").glob(f"*/ripgrep-{RG_VERSION}"), None)
    if source is None:
        raise ValueError("Built ripgrep source licenses are unavailable")
    for name in ("COPYING", "LICENSE-MIT", "UNLICENSE"):
        shutil.copy2(source / name, root / name)
    return root / "bin/rg"


def prepare_ripgrep(binary, licenses, scratch, node_os, arch, supplied=None):
    """Always bundle a standalone build; never silently reuse a Homebrew-linked rg."""
    binary.parent.mkdir(parents=True, exist_ok=True)
    licenses.mkdir(parents=True, exist_ok=True)
    if supplied:
        supplied = Path(supplied).resolve()
        license_names = ("COPYING", "LICENSE-MIT", "UNLICENSE")
        license_root = next((root for root in (supplied.parent, supplied.parent.parent) if all((root / name).is_file() for name in license_names)), supplied.parent)
        required = [license_root / name for name in license_names]
        if not all(path.is_file() for path in required):
            raise ValueError("Supplied ripgrep needs sibling COPYING, LICENSE-MIT and UNLICENSE in its distribution root")
        shutil.copy2(supplied, binary)
        for path in required:
            shutil.copy2(path, licenses / path.name)
    else:
        target = ("aarch64" if arch == "arm64" else "x86_64") + ("-apple-darwin" if node_os == "darwin" else "-unknown-linux-musl")
        name = f"ripgrep-{RG_VERSION}-{target}.tar.gz"
        cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "kyler/build-runtime"
        cache.mkdir(parents=True, exist_ok=True)
        archive = cache / name
        expected = RG_DIGESTS[target]
        if not archive.is_file() or digest(archive) != expected:
            stage = scratch / name
            print(f"Downloading pinned ripgrep {RG_VERSION} ({target})", flush=True)
            try:
                fetch(f"https://github.com/BurntSushi/ripgrep/releases/download/{RG_VERSION}/{name}", stage)
            except (urllib.error.URLError, subprocess.CalledProcessError):
                # Linux builds target musl; macOS source builds avoid dynamic Homebrew PCRE2.
                build_target = target if node_os == "linux" else None
                return prepare_ripgrep(binary, licenses, scratch, node_os, arch, build_ripgrep(scratch, build_target))
            if digest(stage) != expected:
                raise ValueError("ripgrep archive checksum mismatch")
            shutil.copyfile(stage, archive)
        unpack = scratch / "ripgrep"
        unpack.mkdir()
        extract_regular_archive(archive, unpack)
        root = unpack / name.removesuffix(".tar.gz")
        shutil.copy2(root / "rg", binary)
        for name in ("COPYING", "LICENSE-MIT", "UNLICENSE"):
            shutil.copy2(root / name, licenses / name)
    binary.chmod(0o755)
    version = check_ripgrep(binary, node_os)
    print(f"Bundled {version}", flush=True)
    return {"version": version, "sha256": digest(binary)}
