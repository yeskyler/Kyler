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

    def invoke(self, arguments=(), releases=None, remote="", dirty=False, local_tag=None, fail_upload=False, remote_assets=None, remote_sums="", remote_installer=None):
        commands = []
        self.commands = commands
        remote_assets = list(remote_assets or [])
        self.uploaded_manifest = None
        def download(repo, asset, destination):
            content = remote_sums.encode() if asset["name"] == "SHA256SUMS" else remote_installer
            if content is None:
                content = (self.dist / asset["name"]).read_bytes()
            destination.write_bytes(content)
        def run(*command):
            commands.append(command)
            if fail_upload and command[:3] in [("gh", "release", "create"), ("gh", "release", "upload")]:
                raise subprocess.CalledProcessError(1, command)
            if command[:3] == ("git", "status", "--porcelain"):
                return " M source" if dirty else ""
            if command[:3] == ("git", "rev-parse", "HEAD"):
                return "commit-id"
            if command[:3] == ("git", "remote", "get-url"):
                return "https://github.com/yeskyler/Kyler.git"
            if command[:2] == ("gh", "api"):
                return json.dumps(remote_assets if "/assets?" in command[-1] else (releases or []))
            if command[:3] == ("gh", "release", "upload"):
                path = Path(command[4])
                if path.name == "SHA256SUMS":
                    self.uploaded_manifest = path.read_text()
                remote_assets[:] = [a for a in remote_assets if a["name"] != path.name]
                remote_assets.append({"id": len(remote_assets) + 100, "name": path.name, "state": "uploaded", "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()})
            if command[:2] == ("git", "ls-remote"):
                return remote
            return ""
        tag_result = subprocess.CompletedProcess([], 0 if local_tag else 1, stdout=(local_tag or "") + "\n")
        with patch.object(release, "download_asset", side_effect=download), patch.object(release, "run", side_effect=run), patch.object(release.subprocess, "run", return_value=tag_result), patch("sys.argv", ["release.py", "0.1.0", "--artifacts", str(self.dist), *arguments]), patch("sys.stdout", new_callable=io.StringIO):
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
        for options, message in [({"dirty": True}, "Commit or stash"), ({"local_tag": "other"}, "local tag"), ({"remote": "other\trefs/tags/v0.1.0"}, "remote tag")]:
            with self.subTest(options=options), self.assertRaisesRegex(ValueError, message):
                self.invoke(**options)

    def existing(self, **kwargs):
        return self.invoke(releases=[{"id": 42, "tag_name": "v0.1.0", "draft": False}], remote="commit-id\trefs/tags/v0.1.0", **kwargs)

    def test_append_preserves_other_platforms_and_merges_checksums(self):
        other = "kyler-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
        digest = "a" * 64
        assets = [
            {"id": 1, "name": "SHA256SUMS", "state": "uploaded"},
            {"id": 2, "name": other, "state": "uploaded", "digest": "sha256:" + digest},
            {"id": 3, "name": "install.sh", "state": "uploaded"},
        ]
        commands = self.existing(remote_assets=assets, remote_sums=f"{digest}  {other}\n")
        uploads = [c for c in commands if c[:3] == ("gh", "release", "upload")]
        self.assertEqual([Path(c[4]).name for c in uploads], [self.name, "SHA256SUMS"])
        self.assertNotIn("--clobber", uploads[0])
        self.assertIn("--clobber", uploads[1])
        self.assertIn(other, self.uploaded_manifest)
        self.assertIn(self.name, self.uploaded_manifest)
        self.assertFalse(any(c[:3] in [("gh", "release", "create"), ("gh", "release", "edit")] for c in commands))
        self.assertFalse(any(c[:2] in [("git", "tag"), ("git", "push")] for c in commands))
        self.assertFalse(any("--slurp" in c for c in commands))

    def test_identical_assets_are_skipped_on_retry(self):
        assets = [{"id": i, "name": name, "state": "uploaded", "digest": "sha256:" + hashlib.sha256((self.dist / name).read_bytes()).hexdigest()} for i, name in enumerate([self.name, "install.sh", "SHA256SUMS"])]
        commands = self.existing(remote_assets=assets, remote_sums=(self.dist / "SHA256SUMS").read_text())
        self.assertFalse(any(c[:3] == ("gh", "release", "upload") for c in commands))

    def test_conflicting_bundle_or_installer_prevents_all_uploads(self):
        for name in (self.name, "install.sh"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "Existing asset differs"):
                self.existing(remote_assets=[{"id": 1, "name": name, "state": "uploaded", "digest": "sha256:" + "0" * 64}])
            self.assertFalse(any(c[:3] == ("gh", "release", "upload") for c in self.commands))

    def test_invalid_remote_manifest_prevents_uploads(self):
        asset = {"id": 1, "name": "SHA256SUMS", "state": "uploaded"}
        with self.assertRaisesRegex(ValueError, "checksum entry"):
            self.existing(remote_assets=[asset], remote_sums="invalid")
        self.assertFalse(any(c[:3] == ("gh", "release", "upload") for c in self.commands))

    def test_append_failure_does_not_replace_manifest(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.existing(fail_upload=True)
        self.assertIsNone(self.uploaded_manifest)

    def test_existing_release_requires_matching_tag(self):
        with self.assertRaisesRegex(ValueError, "remote tag"):
            self.invoke(releases=[{"id": 42, "tag_name": "v0.1.0"}], remote="other\trefs/tags/v0.1.0")
        with self.assertRaisesRegex(ValueError, "tag is missing"):
            self.invoke(releases=[{"id": 42, "tag_name": "v0.1.0"}])
        self.assertFalse(any(c[:2] in [("git", "tag"), ("git", "push")] for c in self.commands))

    def test_missing_manifest_recovers_other_platform_checksums(self):
        other = "kyler-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
        assets = [{"id": 5, "name": other, "state": "uploaded", "digest": "sha256:" + "b" * 64}]
        commands = self.existing(remote_assets=assets)
        self.assertIn("b" * 64 + "  " + other, self.uploaded_manifest)
        self.assertIn(self.name, self.uploaded_manifest)
        manifest = next(c for c in commands if c[:3] == ("gh", "release", "upload") and Path(c[4]).name == "SHA256SUMS")
        self.assertNotIn("--clobber", manifest)

    def test_incomplete_remote_archive_is_not_replaced(self):
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.existing(remote_assets=[{"id": 2, "name": self.name, "state": "starter"}])
        self.assertFalse(any(c[:3] == ("gh", "release", "upload") for c in self.commands))

    def test_older_metadata_downloads_existing_asset_for_verification(self):
        installer = {"id": 5, "name": "install.sh", "state": "uploaded"}
        with self.assertRaisesRegex(ValueError, "Existing asset differs"):
            self.existing(remote_assets=[installer], remote_installer=b"changed installer")
        self.assertFalse(any(c[:3] == ("gh", "release", "upload") for c in self.commands))

    def test_checksum_conflict_is_rejected_before_upload(self):
        assets = [{"id": 1, "name": "SHA256SUMS", "state": "uploaded"},
                  {"id": 2, "name": self.name, "state": "uploaded"}]
        with self.assertRaisesRegex(ValueError, "Existing checksum differs"):
            self.existing(remote_assets=assets, remote_sums="0" * 64 + "  " + self.name + "\n")
        self.assertFalse(any(c[:3] == ("gh", "release", "upload") for c in self.commands))

    def test_existing_draft_is_published_only_when_requested(self):
        for args in ([], ["--draft"]):
            commands = self.invoke(arguments=args, releases=[{"id": 42, "tag_name": "v0.1.0", "draft": True}], remote="commit-id\trefs/tags/v0.1.0")
            edits = [c for c in commands if c[:3] == ("gh", "release", "edit")]
            self.assertEqual(len(edits), 0 if args else 1)

    def test_concurrent_asset_changes_do_not_replace_shared_manifest(self):
        assets = [{"id": i, "name": name, "state": "uploaded", "digest": "sha256:" + hashlib.sha256((self.dist / name).read_bytes()).hexdigest()} for i, name in enumerate([self.name, "install.sh", "SHA256SUMS"])]
        changed = assets + [{"id": 99, "name": "another-upload", "state": "uploaded"}]
        with patch.object(release, "github_pages", side_effect=[[{"id": 42, "tag_name": "v0.1.0"}], assets, changed]):
            with self.assertRaisesRegex(ValueError, "concurrently"):
                self.existing(remote_assets=assets, remote_sums=(self.dist / "SHA256SUMS").read_text())
        self.assertIsNone(self.uploaded_manifest)

    def test_pagination_without_slurp_checks_later_pages(self):
        first = [{"id": i} for i in range(100)]
        with patch.object(release, "run", side_effect=[json.dumps(first), '[{"id": 100}]']) as run:
            result = release.github_pages("repos/owner/repo/releases")
        self.assertEqual(len(result), 101)
        self.assertIn("page=2", run.call_args_list[-1].args[-1])



if __name__ == "__main__":
    unittest.main()
