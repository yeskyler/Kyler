#!/usr/bin/env python3
"""Publish existing native bundles to GitHub Releases; never build or install them."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?")
TARGET = r"(?:aarch64|x86_64)-(?:apple-darwin|unknown-linux-gnu)"


def run(*command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def repository(origin):
    if origin.startswith("git@github.com:"):
        path = origin[len("git@github.com:"):]
    else:
        url = urlsplit(origin)
        if url.scheme != "https" or url.hostname != "github.com":
            raise ValueError("origin must point to a GitHub repository")
        path = url.path.lstrip("/")
    path = path.removesuffix(".git")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", path):
        raise ValueError("Invalid GitHub origin")
    return path


def workspace_version():
    manifest = (ROOT / "kyler-rs/Cargo.toml").read_text()
    section = re.search(r"(?ms)^\[workspace\.package\]\s*\n(.*?)(?=^\[|\Z)", manifest)
    match = re.search(r'^version\s*=\s*"([^"]+)"', section[1], re.M) if section else None
    if not match:
        raise ValueError("Cannot read workspace version")
    return match[1]


def assets(directory, version, revision):
    license_path = ROOT / "LICENSE"
    if not license_path.is_file():
        raise ValueError("Choose and commit a project LICENSE before releasing")
    checksums = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        if not line.strip():
            continue
        match = re.fullmatch(r"([a-fA-F0-9]{64})\s+\*?([^/\\\s]+)", line)
        if not match or match[2] in checksums:
            raise ValueError("Invalid or duplicate release checksum entry")
        checksums[match[2]] = match[1].lower()
    bundles = sorted(directory.glob(f"kyler-v{version}-*.tar.gz"))
    if not bundles:
        raise ValueError(f"No native bundles for version {version}")
    lines = []
    for path in bundles:
        match = re.fullmatch(rf"kyler-v{re.escape(version)}-({TARGET})\.tar\.gz", path.name)
        if not match or path.is_symlink() or not path.is_file():
            raise ValueError("Invalid native bundle filename or file type")
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        if checksums.get(path.name) != digest.hexdigest():
            raise ValueError(f"Checksum mismatch: {path.name}")
        with tarfile.open(path) as archive:
            member = archive.getmember(f"{path.name[:-7]}/bundle.json")
            if not member.isfile() or member.size > 65536:
                raise ValueError("Invalid bundle metadata")
            with archive.extractfile(member) as source:
                metadata = json.load(source)
            if metadata.get("version") != version or metadata.get("target") != match[1]:
                raise ValueError("Bundle version or target does not match its filename")
            if metadata.get("source_revision") != revision:
                raise ValueError("Bundle source revision does not match HEAD; rebuild from the clean release commit")
            license_member = archive.getmember(f"{path.name[:-7]}/LICENSE")
            if not license_member.isfile() or license_member.size != license_path.stat().st_size:
                raise ValueError("Bundle LICENSE differs from source")
            with archive.extractfile(license_member) as source:
                if source.read() != license_path.read_bytes():
                    raise ValueError("Bundle LICENSE differs from source")
        lines.append(f"{digest.hexdigest()}  {path.name}\n")
    installer = directory / "install.sh"
    if installer.is_symlink() or not installer.is_file():
        raise ValueError("Missing release install.sh")
    if installer.read_bytes() != (ROOT / "scripts/install.sh").read_bytes():
        raise ValueError("Release install.sh differs from the current source")
    return bundles, installer, "".join(lines)


def collect_artifacts(source, destination, version, revision):
    """Merge separately downloaded platform artifacts without overwriting assets."""
    if destination.exists():
        raise ValueError("Collected artifact destination must not already exist")
    groups = sorted(path for path in source.iterdir() if path.is_dir())
    if not groups:
        raise ValueError("No platform artifacts to collect")
    files = {}
    checksums = []
    for group in groups:
        bundles, _, sums = assets(group, version, revision)
        for bundle in bundles:
            if bundle.name in files:
                raise ValueError("Duplicate platform archive across artifacts")
            files[bundle.name] = bundle
        checksums.append(sums)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".release-collect-", dir=destination.parent) as temporary:
        stage = Path(temporary) / "assets"
        stage.mkdir()
        for name, path in files.items():
            shutil.copy2(path, stage / name)
        shutil.copy2(ROOT / "scripts/install.sh", stage / "install.sh")
        (stage / "SHA256SUMS").write_text("".join(sorted("".join(checksums).splitlines(keepends=True))))
        stage.rename(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="Release version, for example 0.1.0 or v0.1.0")
    parser.add_argument("--artifacts", type=Path, default=ROOT / "dist")
    parser.add_argument("--collect", type=Path, help="Collect separate platform artifact directories into --artifacts")
    parser.add_argument("--require-target", action="append", default=[], help="Require a native target (repeatable)")
    parser.add_argument("--require-existing-tag", action="store_true", help="Require an existing remote tag; never create or push it")
    parser.add_argument("--draft", action="store_true", help="Upload and retain a draft release")
    parser.add_argument("--dry-run", action="store_true", help="Validate locally; do not authenticate, tag, push or publish")
    args = parser.parse_args()
    version = args.version.removeprefix("v")
    if not VERSION.fullmatch(version) or workspace_version() != version:
        raise ValueError("Release version must match [workspace.package].version")
    if run("git", "status", "--porcelain"):
        raise ValueError("Commit or stash source changes before releasing")
    revision = run("git", "rev-parse", "HEAD")
    repo = repository(run("git", "remote", "get-url", "origin"))
    if args.collect:
        collect_artifacts(args.collect.resolve(), args.artifacts.resolve(), version, revision)
    bundles, installer, sums = assets(args.artifacts.resolve(), version, revision)
    available_targets = {path.name[len(f"kyler-v{version}-"):-7] for path in bundles}
    if not set(args.require_target).issubset(available_targets):
        raise ValueError("A required native target is missing from the release")
    tag = f"v{version}"
    local = subprocess.run(["git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"], cwd=ROOT, text=True, capture_output=True)
    if local.returncode == 0 and local.stdout.strip() != revision:
        raise ValueError("Existing local tag points to a different commit; it will not be moved")
    plan = {"repository": repo, "tag": tag, "commit": revision, "assets": [p.name for p in bundles] + ["SHA256SUMS", "install.sh"], "draft": args.draft, "prerelease": "-" in version.split("+")[0]}
    if args.dry_run:
        print(json.dumps(plan, indent=2))
        return
    run("gh", "auth", "status", "--hostname", "github.com")
    releases = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100"))
    if any(item["tag_name"] == tag for page in releases for item in page):
        raise ValueError("Release already exists; existing releases and assets are never overwritten")
    remote = run("git", "ls-remote", "--tags", "origin", f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}")
    refs = dict(line.split()[::-1] for line in remote.splitlines())
    remote_revision = refs.get(f"refs/tags/{tag}^{{}}", refs.get(f"refs/tags/{tag}"))
    if remote_revision and remote_revision != revision:
        raise ValueError("Existing remote tag points to a different commit; it will not be moved")
    if not remote_revision and args.require_existing_tag:
        raise ValueError("The release tag is missing from origin; it will not be recreated")
    if not remote_revision:
        if local.returncode != 0:
            run("git", "tag", "-a", tag, "-m", f"Kyler {version}")
        run("git", "push", "origin", f"refs/tags/{tag}")
    with tempfile.TemporaryDirectory(prefix="kyler-release-") as temporary:
        manifest = Path(temporary) / "SHA256SUMS"
        manifest.write_text(sums)
        command = ["gh", "release", "create", tag, *(str(p) for p in bundles), str(manifest), str(installer), "--repo", repo, "--verify-tag", "--generate-notes", "--title", f"Kyler {version}", "--draft"]
        if plan["prerelease"]:
            command.append("--prerelease")
        run(*command)
    if not args.draft:
        run("gh", "release", "edit", tag, "--repo", repo, "--draft=false")
    print(f"{'Draft' if args.draft else 'Published'}: https://github.com/{repo}/releases/tag/{tag}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, tarfile.TarError, subprocess.CalledProcessError) as error:
        print(f"Error: {error}", file=sys.stderr)
        print("No existing release was overwritten. If upload failed, inspect the tag and draft before retrying.", file=sys.stderr)
        sys.exit(1)
