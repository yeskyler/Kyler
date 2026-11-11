#!/usr/bin/env python3
"""Isolated packaging setup tests; never install toolchains or use the network."""
import argparse
import importlib.util
import contextlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("package", Path(__file__).resolve().parents[1] / "package.py")
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class PackageSetupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.environment = {"PATH": str(self.bin), "CARGO_HOME": str(self.root / "cargo")}

    def tool(self, name, directory=None):
        path = (directory or self.bin) / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("#!/bin/sh\nexit 0\n")
        path.chmod(0o755)

    def test_existing_rust_is_checked_without_installing(self):
        self.tool("rustc")
        self.tool("cargo")
        with patch.object(package.subprocess, "run") as run:
            package.prepare_rust(self.environment, self.root)
        self.assertEqual([call.args[0] for call in run.call_args_list], [["rustc", "--version"], ["cargo", "--version"]])

    def test_saved_cargo_home_is_used_without_installing(self):
        saved_bin = Path(self.environment["CARGO_HOME"]) / "bin"
        self.tool("rustc", saved_bin)
        self.tool("cargo", saved_bin)
        with patch.object(package.subprocess, "run") as run:
            package.prepare_rust(self.environment, self.root)
        self.assertEqual(len(run.call_args_list), 2)
        self.assertEqual(shutil.which("cargo", path=self.environment["PATH"]), str((saved_bin / "cargo").resolve()))

    def test_missing_rust_installs_and_checks_tools_in_current_process(self):
        self.tool("curl")
        def install(command, **kwargs):
            if command[0] == "sh":
                for tool in ("rustc", "cargo"):
                    self.tool(tool, Path(self.environment["CARGO_HOME"]) / "bin")
        with patch.object(package.subprocess, "run", side_effect=install) as run:
            package.prepare_rust(self.environment, self.root)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertIn("https://sh.rustup.rs", commands[0])
        self.assertEqual(commands[1][2:], ["-y", "--profile", "minimal", "--default-toolchain", "stable", "--no-modify-path"])
        self.assertEqual(commands[-2:], [["rustc", "--version"], ["cargo", "--version"]])
        for call in run.call_args_list:
            self.assertTrue(call.kwargs["check"])
            self.assertIs(call.kwargs["env"], self.environment)

    def test_failed_rust_download_stops_before_install(self):
        self.tool("curl")
        with patch.object(package.subprocess, "run", side_effect=subprocess.CalledProcessError(22, "curl")) as run:
            with self.assertRaises(subprocess.CalledProcessError):
                package.prepare_rust(self.environment, self.root)
        self.assertEqual(run.call_count, 1)

    def test_missing_curl_has_actionable_error(self):
        with self.assertRaisesRegex(ValueError, "curl is required"):
            package.prepare_rust(self.environment, self.root)

    def test_local_node_and_license_are_reused_without_download(self):
        self.tool("node")
        self.tool("npm")
        (self.root / "LICENSE").write_text("Node license")
        scratch = self.root / "scratch"
        scratch.mkdir()
        details = '{"version":"v24.0.0","platform":"linux","arch":"x64"}'
        with patch.dict(package.os.environ, self.environment, clear=True), patch.object(package.subprocess, "check_output", return_value=details), patch.object(package, "download") as fetch:
            result = package.node_distribution(argparse.Namespace(node_dist=None, skip_build=False), scratch, "linux", "x64")
        fetch.assert_not_called()
        self.assertEqual((result / "bin/node").resolve(), (self.bin / "node").resolve())
        self.assertEqual((result / "LICENSE").read_text(), "Node license")

    def test_old_local_node_falls_back_to_download(self):
        self.tool("node")
        details = '{"version":"v20.0.0","platform":"linux","arch":"x64"}'
        with patch.dict(package.os.environ, self.environment, clear=True), patch.object(package.subprocess, "check_output", return_value=details):
            self.assertIsNone(package.local_node(argparse.Namespace(skip_build=False), self.root, "linux", "x64"))

    def test_missing_local_npm_stops_with_actionable_error(self):
        self.tool("node")
        details = '{"version":"v24.0.0","platform":"linux","arch":"x64"}'
        with patch.dict(package.os.environ, self.environment, clear=True), patch.object(package.subprocess, "check_output", return_value=details):
            with self.assertRaisesRegex(ValueError, "npm is missing"):
                package.local_node(argparse.Namespace(skip_build=False), self.root, "linux", "x64")

    def test_local_node_downloads_only_matching_license_if_missing(self):
        self.tool("node")
        self.tool("npm")
        details = '{"version":"v24.0.0","platform":"linux","arch":"x64"}'
        def license_download(url, destination):
            destination.write_text("Node license")
        with patch.dict(package.os.environ, self.environment, clear=True), patch.object(package.subprocess, "check_output", return_value=details), patch.object(package, "download", side_effect=license_download) as fetch:
            result = package.local_node(argparse.Namespace(skip_build=False), self.root, "linux", "x64")
        self.assertTrue((result / "LICENSE").is_file())
        self.assertEqual(fetch.call_args.args[0], "https://raw.githubusercontent.com/nodejs/node/v24.0.0/LICENSE")

    def test_skip_build_packages_fixture_with_all_nine_steps(self):
        project = self.root / "project"
        web = project / "web"
        (web / ".next/standalone").mkdir(parents=True)
        (web / ".next/static").mkdir()
        (web / ".next/standalone/server.js").write_text("server")
        (web / "README.md").write_text("Web")
        (web / "package-lock.json").write_text('{"packages":{}}')
        (project / "docs").mkdir()
        (project / "scripts").mkdir()
        self.tool("kyler")
        distribution = self.root / "runtime"
        self.tool("node", distribution / "bin")
        (distribution / "LICENSE").write_text("Node license")
        output = self.root / "dist"
        log = io.StringIO()
        def version(command, **kwargs):
            if Path(command[0]).name == "node":
                return json.dumps({"version": "v24.0.0", "platform": "linux", "arch": "x64"})
            return "kyler 0.1.0"
        with patch.object(package, "ROOT", project), patch.object(package, "WEB", web), patch.object(package.platform, "system", return_value="Linux"), patch.object(package.platform, "machine", return_value="x86_64"), patch.object(package.subprocess, "check_output", side_effect=version), patch.object(package.subprocess, "run") as run, patch("sys.argv", ["package.py", "--skip-build", "--node-dist", str(distribution), "--binary", str(self.bin / "kyler"), "--output", str(output)]), contextlib.redirect_stdout(log):
            package.main()
        run.assert_not_called()
        text = log.getvalue()
        positions = [text.index(f"[{number}/9]") for number in range(1, 10)]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("Packaging complete", text)
        archive = output / "kyler-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
        self.assertFalse((output / "install.sh").exists())
        self.assertIn(package.digest(archive), (output / "SHA256SUMS").read_text())
        with tarfile.open(archive) as bundle:
            prefix = archive.name[:-7]
            self.assertNotIn(prefix + "/install.sh", bundle.getnames())
            self.assertEqual(bundle.extractfile(prefix + "/runtime/node").read(), (distribution / "bin/node").read_bytes())
            self.assertEqual(bundle.extractfile(prefix + "/licenses/NODE-LICENSE").read(), b"Node license")

    def test_default_node_download_is_verified_and_includes_npm(self):
        distribution = self.root / "node-v24.0.0-linux-x64"
        self.tool("node", distribution / "bin")
        self.tool("npm", distribution / "bin")
        (distribution / "LICENSE").write_text("Test license")
        archive = self.root / (distribution.name + ".tar.gz")
        with tarfile.open(archive, "w:gz") as output:
            output.add(distribution, arcname=distribution.name)
        def download(url, destination):
            if url.endswith("SHASUMS256.txt"):
                destination.write_text(f"{package.digest(archive)}  {archive.name}\n")
            else:
                shutil.copy2(archive, destination)
        scratch = self.root / "scratch"
        scratch.mkdir()
        with patch.object(package, "download", side_effect=download) as fetch:
            result = package.node_distribution(argparse.Namespace(node_dist=None, download_node=True), scratch, "linux", "x64")
        self.assertTrue((result / "bin/node").is_file())
        self.assertTrue((result / "bin/npm").is_file())
        self.assertEqual(fetch.call_count, 2)

    def test_node_checksum_mismatch_stops_before_unpacking(self):
        def download(url, destination):
            if url.endswith("SHASUMS256.txt"):
                destination.write_text("0" * 64 + "  node-v24.0.0-linux-x64.tar.gz\n")
            else:
                destination.write_bytes(b"corrupt archive")
        with patch.object(package, "download", side_effect=download):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                package.node_distribution(argparse.Namespace(node_dist=None, download_node=True), self.root, "linux", "x64")
        self.assertFalse((self.root / "node").exists())


if __name__ == "__main__":
    unittest.main()
