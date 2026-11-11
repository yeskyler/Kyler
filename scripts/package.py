#!/usr/bin/env python3
"""Build a native release bundle. No end-user npm/Cargo installation is needed."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
RUST = ROOT / "kyler-rs"
WEB = ROOT / "web"


def digest(path):
    with path.open("rb") as source:
        result = hashlib.sha256()
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
        return result.hexdigest()


def download(url, destination):
    if not url.startswith("https://"):
        raise ValueError("Release downloads must use HTTPS")
    print(f"Downloading: {url}", flush=True)
    with urllib.request.urlopen(url, timeout=120) as source, destination.open("wb") as target:
        if not source.url.startswith("https://"):
            raise ValueError("Insecure download redirect")
        received = 0
        next_report = 8 * 1024 * 1024
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            target.write(chunk)
            received += len(chunk)
            if received >= next_report:
                print(f"  Downloaded {received / (1024 * 1024):.1f} MiB", flush=True)
                next_report = received + 8 * 1024 * 1024
        print(f"  Download complete: {received} bytes", flush=True)


def step(number, message):
    print(f"\n[{number}/9] {message}", flush=True)


def local_node(args, temporary, node_os, arch):
    node = shutil.which("node")
    if not node:
        print("Local Node.js: not found.", flush=True)
        return None
    try:
        details = json.loads(subprocess.check_output([
            node, "-p", "JSON.stringify({version:process.version,platform:process.platform,arch:process.arch})",
        ], text=True))
    except (OSError, subprocess.CalledProcessError, ValueError):
        print(f"Local Node.js cannot run: {node}", flush=True)
        return None
    print(f"Local Node.js: {details['version']} ({details['platform']}/{details['arch']}) at {node}", flush=True)
    if details["platform"] != node_os or details["arch"] != arch or int(details["version"].split(".")[0][1:]) < 22:
        print("Local Node.js requires native architecture and version 22 or newer.", flush=True)
        return None
    if not args.skip_build and not shutil.which("npm"):
        raise ValueError("Local Node.js is available but npm is missing; install npm or use --node-dist")
    distribution = temporary / "local-node"
    (distribution / "bin").mkdir(parents=True)
    # Retain the local npm command and its installation layout; only node is bundled.
    (distribution / "bin/node").symlink_to(Path(node).resolve())
    prefix = Path(node).resolve().parent.parent
    candidates = [prefix / "LICENSE", prefix / "share/doc/node/LICENSE", prefix / "share/licenses/node/LICENSE", prefix / "include/node/LICENSE"]
    license_path = next((path for path in candidates if path.is_file()), None)
    if license_path:
        shutil.copy2(license_path, distribution / "LICENSE")
        print(f"Reusing Node.js license: {license_path}", flush=True)
    else:
        print("Local Node.js license is missing; downloading its matching license only (no runtime download).", flush=True)
        download(f"https://raw.githubusercontent.com/nodejs/node/{details['version']}/LICENSE", distribution / "LICENSE")
    print("Reusing local Node.js; runtime download skipped.", flush=True)
    return distribution


def node_distribution(args, temporary, node_os, arch):
    if args.node_dist:
        print(f"Using supplied Node.js distribution: {args.node_dist.resolve()}", flush=True)
        return args.node_dist.resolve()
    if not getattr(args, "download_node", False):
        local = local_node(args, temporary, node_os, arch)
        if local:
            return local
    print("Preparing the official Node.js 24 distribution...", flush=True)
    base = "https://nodejs.org/dist/latest-v24.x"
    sums = temporary / "node-checksums"
    download(f"{base}/SHASUMS256.txt", sums)
    suffix = f"-{node_os}-{arch}.tar.gz"
    matches = [(line.split()[0], line.split()[1]) for line in sums.read_text().splitlines() if line.split()[1].endswith(suffix)]
    if len(matches) != 1:
        raise ValueError("No unique official Node runtime for this platform")
    expected, filename = matches[0]
    archive = temporary / filename
    download(f"{base}/{filename}", archive)
    if digest(archive) != expected:
        raise ValueError("Node runtime checksum mismatch")
    print("Node.js archive checksum verified. Extracting runtime...", flush=True)
    with tarfile.open(archive) as source:
        destination = (temporary / "node").resolve()
        for member in source.getmembers():
            path = (destination / member.name).resolve()
            if member.name.startswith("/") or not path.is_relative_to(destination):
                raise ValueError("Unsafe Node archive path")
            if member.issym() or member.islnk():
                target = (path.parent if member.issym() else destination) / member.linkname
                if not target.resolve().is_relative_to(destination):
                    raise ValueError("Unsafe Node archive link")
            elif not member.isfile() and not member.isdir():
                raise ValueError("Unsupported Node archive member")
        source.extractall(destination)
    return temporary / "node" / filename.removesuffix(".tar.gz")


def prepare_rust(environment, temporary):
    """Reuse available tools or install rustup without changing shell profiles."""
    def available():
        return all(shutil.which(tool, path=environment.get("PATH", "")) for tool in ("rustc", "cargo"))

    if not available():
        cargo_home = Path(environment.get("CARGO_HOME") or Path.home() / ".cargo").expanduser().resolve()
        environment["PATH"] = str(cargo_home / "bin") + os.pathsep + environment.get("PATH", "")
    if not available():
        print("Rust is missing. Installing the stable toolchain with rustup...", flush=True)
        if not shutil.which("curl", path=environment.get("PATH", "")):
            raise ValueError("curl is required to install Rust")
        installer = temporary / "rustup-init.sh"
        subprocess.run([
            "curl", "--proto", "=https", "--tlsv1.2", "-sSf",
            "https://sh.rustup.rs", "-o", str(installer),
        ], env=environment, check=True)
        subprocess.run([
            "sh", str(installer), "-y", "--profile", "minimal",
            "--default-toolchain", "stable", "--no-modify-path",
        ], env=environment, check=True)
    print("Checking Rust toolchain versions (reusing installed tools).", flush=True)
    for tool in ("rustc", "cargo"):
        if not shutil.which(tool, path=environment.get("PATH", "")):
            raise ValueError(f"{tool} is unavailable after Rust setup")
        subprocess.run([tool, "--version"], env=environment, check=True)


def source_revision():
    """Identify clean source; reused builds cannot establish their source revision."""
    try:
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, stderr=subprocess.DEVNULL).strip()
        dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True).strip()
        return revision if not dirty else None
    except (OSError, subprocess.CalledProcessError):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node-dist", type=Path, help="Extracted official Node distribution (bin/node and LICENSE)")
    parser.add_argument("--download-node", action="store_true", help="Download official Node.js 24 instead of reusing local Node.js")
    parser.add_argument("--skip-build", action="store_true", help="Use an already-built backend and web/.next/standalone")
    parser.add_argument("--binary", type=Path, default=RUST / "target/release/kyler")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    revision = source_revision() if not args.skip_build and args.binary.resolve() == (RUST / "target/release/kyler").resolve() else None
    node_os = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "x64", "AMD64": "x64"}.get(platform.machine())
    if not node_os or not arch:
        raise ValueError("Build on a supported native macOS/Linux host")
    target = ("aarch64" if arch == "arm64" else "x86_64") + ("-apple-darwin" if node_os == "darwin" else "-unknown-linux-gnu")
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kyler-package-") as scratch:
        temporary = Path(scratch)
        environment = os.environ.copy()
        print(f"Target: {target}\nOutput: {args.output.resolve()}", flush=True)
        step(1, "Check Rust environment")
        if args.skip_build:
            print("Skipped: using existing backend and Web build.", flush=True)
        if not args.skip_build:
            prepare_rust(environment, temporary)
        step(2, "Check Node.js and npm environment")
        distribution = node_distribution(args, temporary, node_os, arch)
        node = distribution / "bin/node"
        license_path = distribution / "LICENSE"
        if not node.is_file() or not license_path.is_file():
            raise ValueError("Node distribution must contain bin/node and LICENSE")
        details = json.loads(subprocess.check_output([node, "-p", "JSON.stringify({version:process.version,platform:process.platform,arch:process.arch})"], text=True))
        if details["platform"] != node_os or details["arch"] != arch or int(details["version"].split(".")[0][1:]) < 22:
            raise ValueError("Node runtime must be native and version 22 or newer")
        environment["PATH"] = str(distribution / "bin") + os.pathsep + environment.get("PATH", "")
        environment["NEXT_TELEMETRY_DISABLED"] = "1"
        print(f"Node.js {details['version']} ({details['platform']}/{details['arch']})", flush=True)
        if not args.skip_build:
            if not shutil.which("npm", path=environment["PATH"]):
                raise ValueError("The Node.js distribution must provide npm for building the Web UI")
            subprocess.run(["npm", "--version"], env=environment, check=True)
            step(3, "Build Rust backend")
            subprocess.run(["cargo", "build", "--locked", "--release", "-p", "kyler-cli"], cwd=RUST, env=environment, check=True)
            step(4, "Install Web dependencies (npm ci)")
            subprocess.run(["npm", "ci"], cwd=WEB, env=environment, check=True)
            step(5, "Build Web production bundle")
            subprocess.run(["npm", "run", "build"], cwd=WEB, env=environment, check=True)
        else:
            for number, label in [(3, "Rust backend"), (4, "Web dependencies"), (5, "Web production bundle")]:
                step(number, f"Skip {label}: --skip-build")
        step(6, "Collect backend, Node.js, Web, documentation and licenses")
        binary = args.binary.resolve()
        reported = subprocess.check_output([binary, "--version"], text=True, env={k: v for k, v in environment.items() if k not in ("KYLER_INTERNAL_TOOL_WORKER", "KYLER_STARTUP_TRACE")}).strip()
        if not reported.startswith("kyler "):
            raise ValueError("Unexpected backend version")
        version = reported.split()[1]
        if not version or any(c not in "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.+-" for c in version):
            raise ValueError("Invalid backend version")
        name = f"kyler-v{version}-{target}"
        bundle = temporary / name
        (bundle / "bin").mkdir(parents=True)
        (bundle / "runtime").mkdir()
        shutil.copy2(binary, bundle / "bin/kyler")
        shutil.copy2(node, bundle / "runtime/node")
        (bundle / "licenses").mkdir()
        shutil.copy2(license_path, bundle / "licenses/NODE-LICENSE")
        standalone = WEB / ".next/standalone"
        if not (standalone / "server.js").is_file():
            raise ValueError("Build web/.next/standalone/server.js first")
        # Dereference tracing links so installation never needs to extract archive links.
        shutil.copytree(standalone, bundle / "web", symlinks=False, ignore=shutil.ignore_patterns(".env", ".env.*"))
        shutil.copytree(WEB / ".next/static", bundle / "web/.next/static", dirs_exist_ok=True)
        if (WEB / "public").exists():
            shutil.copytree(WEB / "public", bundle / "web/public", dirs_exist_ok=True)
        for relative in ("LICENSE", "README.md", "README.zh-CN.md", "AGENTS.md", "CONTRIBUTING.md", "SECURITY.md", "CHANGELOG.md"):
            if (ROOT / relative).is_file():
                shutil.copy2(ROOT / relative, bundle / relative)
        shutil.copytree(ROOT / "docs", bundle / "docs")
        shutil.copy2(WEB / "README.md", bundle / "web/README.md")
        lock = json.loads((WEB / "package-lock.json").read_text())
        notices = [{"package": path, "version": item.get("version"), "license": item.get("license")} for path, item in lock.get("packages", {}).items() if path and not item.get("dev")]
        (bundle / "licenses/npm-packages.json").write_text(json.dumps(notices, indent=2) + "\n")
        step(7, "Write bundle metadata and internal checksums")
        (bundle / "bundle.json").write_text(json.dumps({"version": version, "target": target, "source_revision": revision if revision == source_revision() else None, "node": details["version"], "web": "standalone", "license_note": "Project distribution licensing must be selected by the owner before public release." if not (ROOT / "LICENSE").exists() else None}, indent=2) + "\n")
        files = sorted(path for path in bundle.rglob("*") if path.is_file())
        if any("\n" in str(path) or "\r" in str(path) for path in files):
            raise ValueError("Bundle filenames cannot contain line breaks")
        (bundle / "SHA256SUMS").write_text("".join(f"{digest(path)}  {path.relative_to(bundle).as_posix()}\n" for path in files))
        step(8, "Compress native archive")
        output = args.output / f"{name}.tar.gz"
        with tempfile.NamedTemporaryFile(dir=args.output, prefix=".package-", delete=False) as stage:
            stage_path = Path(stage.name)
        try:
            with tarfile.open(stage_path, "w:gz", dereference=True) as archive:
                archive.add(bundle, arcname=name)
            stage_path.replace(output)
        finally:
            stage_path.unlink(missing_ok=True)
        step(9, "Write release checksums")
        sums = args.output / "SHA256SUMS"
        lines = sums.read_text().splitlines() if sums.exists() else []
        lines = [line for line in lines if line.split()[-1] != output.name]
        sums.write_text("\n".join(sorted([*lines, f"{digest(output)}  {output.name}"])) + "\n")
        print(f"\nPackaging complete.\nBundle: {output}\nSize: {output.stat().st_size / (1024 * 1024):.1f} MiB", flush=True)
        print(f"Install: sh scripts/install.sh --bundle {output}")


if __name__ == "__main__":
    main()
