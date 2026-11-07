"""Isolated release tests. No GitHub requests, tag pushes, or public releases."""
import hashlib
import importlib.util
import io
import json
import shutil
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parents[1] / "release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="kyler-release-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "scripts").mkdir()
        (self.root / "scripts/install.sh").write_text("#!/bin/sh\nexit 0\n")
        (self.root / "kyler-rs").mkdir()
        (self.root / "kyler-rs/Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
        (self.root / "LICENSE").write_text("Fixture project license")
        self.dist = self.root / "dist"
        self.dist.mkdir()
        (self.dist / "install.sh").write_bytes((self.root / "scripts/install.sh").read_bytes())
        self.name = "kyler-v0.1.0-aarch64-apple-darwin.tar.gz"
        self.bundle()
        self.patch = patch.object(release, "ROOT", self.root)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def bundle(self, version="0.1.0", target="aarch64-apple-darwin"):
        body = json.dumps({"version": version, "target": target, "source_revision":"commit-id"}).encode()
        with tarfile.open(self.dist / self.name, "w:gz") as archive:
            item = tarfile.TarInfo(self.name[:-7] + "/bundle.json")
            item.size = len(body)
            archive.addfile(item, io.BytesIO(body))
            license_body = (self.root / "LICENSE").read_bytes()
            license_item = tarfile.TarInfo(self.name[:-7] + "/LICENSE")
            license_item.size = len(license_body)
            archive.addfile(license_item, io.BytesIO(license_body))
        digest = hashlib.sha256((self.dist / self.name).read_bytes()).hexdigest()
        (self.dist / "SHA256SUMS").write_text(f"{digest}  {self.name}\n")

    def test_assets_verify_checksum_metadata_and_installer(self):
        bundles, installer, sums = release.assets(self.dist, "0.1.0", "commit-id")
        self.assertEqual([item.name for item in bundles], [self.name])
        self.assertEqual(installer.name, "install.sh")
        self.assertIn(self.name, sums)
        with (self.dist / self.name).open("ab") as source:
            source.write(b"corrupted")
        with self.assertRaisesRegex(ValueError, "Checksum"):
            release.assets(self.dist, "0.1.0", "commit-id")

    def test_wrong_bundle_version_and_installer_are_rejected(self):
        self.bundle("0.2.0")
        with self.assertRaisesRegex(ValueError, "version or target"):
            release.assets(self.dist, "0.1.0", "commit-id")
        self.bundle()
        (self.dist / "install.sh").write_text("different")
        with self.assertRaisesRegex(ValueError, "differs"):
            release.assets(self.dist, "0.1.0", "commit-id")

    def test_release_requires_matching_source_and_license(self):
        with self.assertRaisesRegex(ValueError, "source revision"):
            release.assets(self.dist, "0.1.0", "another-commit")
        (self.root / "LICENSE").write_text("Changed license")
        with self.assertRaisesRegex(ValueError, "LICENSE differs"):
            release.assets(self.dist, "0.1.0", "commit-id")
        (self.root / "LICENSE").unlink()
        with self.assertRaisesRegex(ValueError, "Choose and commit"):
            release.assets(self.dist, "0.1.0", "commit-id")

    def test_collect_merges_platform_checksums_without_collisions(self):
        source=self.root / "platforms"
        source.mkdir()
        shutil.copytree(self.dist,source / "macos")
        self.name="kyler-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
        self.bundle(target="x86_64-unknown-linux-gnu")
        (self.dist / "kyler-v0.1.0-aarch64-apple-darwin.tar.gz").unlink()
        shutil.copytree(self.dist,source / "linux")
        destination=self.root / "collected"
        release.collect_artifacts(source,destination,"0.1.0","commit-id")
        bundles,_,sums=release.assets(destination,"0.1.0","commit-id")
        self.assertEqual(len(bundles),2)
        self.assertEqual(len(sums.splitlines()),2)
        with self.assertRaisesRegex(ValueError,"must not already exist"):
            release.collect_artifacts(source,destination,"0.1.0","commit-id")
        shutil.copytree(source / "macos",source / "duplicate")
        with self.assertRaisesRegex(ValueError,"Duplicate platform"):
            release.collect_artifacts(source,self.root / "duplicates","0.1.0","commit-id")

    def test_repository_and_version_validation(self):
        for origin in ["https://github.com/yeskyler/Kyler.git", "git@github.com:yeskyler/Kyler.git"]:
            self.assertEqual(release.repository(origin), "yeskyler/Kyler")
        with self.assertRaises(ValueError):
            release.repository("https://example.com/yeskyler/Kyler.git")
        self.assertEqual(release.workspace_version(), "0.1.0")

    def invoke(self, arguments=(), releases=None, remote="", dirty=False, local_tag=None, fail_upload=False):
        commands = []
        self.commands = commands
        def run(*command):
            commands.append(command)
            if fail_upload and command[:3] == ("gh", "release", "create"):
                raise subprocess.CalledProcessError(1, command)
            if command[:3] == ("git", "status", "--porcelain"):
                return " M source" if dirty else ""
            if command[:3] == ("git", "rev-parse", "HEAD"):
                return "commit-id"
            if command[:3] == ("git", "remote", "get-url"):
                return "https://github.com/yeskyler/Kyler.git"
            if command[:2] == ("gh", "api"):
                return json.dumps([releases or []])
            if command[:2] == ("git", "ls-remote"):
                return remote
            return ""
        tag_result = subprocess.CompletedProcess([], 0 if local_tag else 1, stdout=(local_tag or "") + "\n")
        with patch.object(release, "run", side_effect=run), patch.object(release.subprocess, "run", return_value=tag_result), patch("sys.argv", ["release.py", "0.1.0", "--artifacts", str(self.dist), *arguments]), patch("sys.stdout", new_callable=io.StringIO):
            release.main()
        return commands

    def test_dry_run_never_calls_github_or_mutates_git(self):
        commands = self.invoke(["--dry-run"])
        self.assertFalse(any(command[0] == "gh" for command in commands))
        self.assertFalse(any(command[:2] in [("git", "tag"), ("git", "push")] for command in commands))

    def test_publish_uploads_to_draft_before_publishing(self):
        commands = self.invoke()
        create = next(command for command in commands if command[:3] == ("gh", "release", "create"))
        self.assertIn("--draft", create)
        self.assertIn("--verify-tag", create)
        self.assertTrue(any(command[:2] == ("git", "push") for command in commands))
        self.assertEqual(commands[-1][:4], ("gh", "release", "edit", "v0.1.0"))

    def test_draft_does_not_publish_and_existing_remote_tag_is_not_pushed(self):
        commands = self.invoke(["--draft"], remote="commit-id\trefs/tags/v0.1.0^{}")
        self.assertFalse(any(command[:2] == ("git", "push") for command in commands))
        self.assertFalse(any(command[:3] == ("gh", "release", "edit") for command in commands))

    def test_workflow_requires_all_targets_and_never_recreates_deleted_tag(self):
        with self.assertRaisesRegex(ValueError,"required native target"):
            self.invoke(["--require-target","x86_64-unknown-linux-gnu"])
        with self.assertRaisesRegex(ValueError,"will not be recreated"):
            self.invoke(["--require-existing-tag"])
        self.assertFalse(any(command[:2] in [("git","tag"),("git","push")] for command in self.commands))
        commands=self.invoke(["--require-existing-tag"],remote="commit-id\trefs/tags/v0.1.0")
        self.assertFalse(any(command[:2] in [("git","tag"),("git","push")] for command in commands))

    def test_upload_failure_never_publishes(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.invoke(fail_upload=True)
        self.assertFalse(any(command[:3] == ("gh", "release", "edit") for command in self.commands))

    def test_dirty_tree_existing_release_and_mismatched_tags_are_rejected(self):
        for options, message in [({"dirty": True}, "Commit or stash"), ({"releases": [{"tag_name": "v0.1.0"}]}, "Release already exists"), ({"local_tag": "other"}, "local tag"), ({"remote": "other\trefs/tags/v0.1.0"}, "remote tag")]:
            with self.subTest(options=options), self.assertRaisesRegex(ValueError, message):
                self.invoke(**options)


if __name__ == "__main__":
    unittest.main()
