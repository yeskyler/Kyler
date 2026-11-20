#!/usr/bin/env python3
"""Prepare the same private runtime beside a Cargo binary; never use a host rg fallback."""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import tempfile

from package_runtime import prepare_linux_sandbox, prepare_ripgrep

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("debug", "release"), default="debug")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--ripgrep-binary", type=Path)
    parser.add_argument("--sandbox-dist", type=Path)
    args = parser.parse_args()
    node_os = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "x64"}.get(platform.machine())
    if not node_os or not arch:
        raise ValueError("Native runtime preparation supports macOS/Linux ARM64 and x86_64")
    destination = (args.output or ROOT / "kyler-rs/target" / args.profile / "runtime").resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".runtime-build-", dir=destination.parent) as temporary:
        scratch = Path(temporary)
        payload = scratch / "runtime"
        info = {"ripgrep": prepare_ripgrep(payload / "tools/rg", payload / "licenses/ripgrep", scratch, node_os, arch, args.ripgrep_binary)}
        if node_os == "linux":
            info["sandbox"] = prepare_linux_sandbox(payload / "sandbox", args.sandbox_dist)
        (payload / "components.json").write_text(json.dumps(info, indent=2)+"\n")
        backup = scratch / "previous"
        if destination.exists():
            destination.rename(backup)
        try:
            payload.rename(destination)
        except Exception:
            if backup.exists():
                backup.rename(destination)
            raise
    print(f"Private runtime ready: {destination}")
    print("Kyler automatically uses these tools; no host rg or sandbox helper fallback.")


if __name__ == "__main__":
    main()
