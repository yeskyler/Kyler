"""Offline tests for third-party payload integrity, linkage and Linux relocation."""
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("package_runtime", Path(__file__).resolve().parents[1] / "package_runtime.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class RuntimePackagingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="kyler runtime tests ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_pinned_download_rejects_corruption_before_extracting_or_executing(self):
        scratch = self.root / "scratch"
        scratch.mkdir()
        def corrupt(url, path):
            path.write_bytes(b"corrupt")
        with patch.dict(runtime.os.environ, {"XDG_CACHE_HOME": str(self.root / "cache")}), patch.object(runtime, "fetch", side_effect=corrupt), patch.object(runtime, "check_ripgrep") as check:
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                runtime.prepare_ripgrep(self.root / "tools/rg", self.root / "licenses", scratch, "darwin", "arm64")
        check.assert_not_called()
        self.assertFalse((self.root / "tools/rg").exists())

    def test_ripgrep_cache_and_upstream_licenses_are_packaged_without_network(self):
        name = f"ripgrep-{runtime.RG_VERSION}-aarch64-apple-darwin"
        upstream = self.root / name
        upstream.mkdir()
        for filename in ("rg", "COPYING", "LICENSE-MIT", "UNLICENSE"):
            (upstream / filename).write_text(filename)
        cache = self.root / "cache/kyler/build-runtime"
        cache.mkdir(parents=True)
        archive = cache / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            output.add(upstream, arcname=name)
        scratch = self.root / "scratch"
        scratch.mkdir()
        with patch.dict(runtime.os.environ, {"XDG_CACHE_HOME": str(self.root / "cache")}), patch.dict(runtime.RG_DIGESTS, {"aarch64-apple-darwin": runtime.digest(archive)}), patch.object(runtime, "fetch") as fetch, patch.object(runtime, "check_ripgrep", return_value="ripgrep fixture"):
            info = runtime.prepare_ripgrep(self.root / "tools/rg", self.root / "licenses", scratch, "darwin", "arm64")
        fetch.assert_not_called()
        self.assertEqual(info["sha256"], runtime.digest(self.root / "tools/rg"))
        self.assertEqual((self.root / "licenses/LICENSE-MIT").read_text(), "LICENSE-MIT")

    def test_mac_ripgrep_must_not_link_homebrew_pcre(self):
        for path, allowed in [("/usr/lib/libSystem.B.dylib", True), ("/opt/homebrew/opt/pcre2/lib/libpcre2.dylib", False), ("@rpath/libpcre2.dylib", False)]:
            with self.subTest(path=path), patch.object(runtime.subprocess, "check_output", side_effect=["ripgrep fixture\n", f"rg:\n\t{path} (compatibility version 1.0.0)\n"]):
                if allowed:
                    runtime.check_ripgrep(Path("rg"), "darwin")
                else:
                    with self.assertRaisesRegex(ValueError, "external libraries"):
                        runtime.check_ripgrep(Path("rg"), "darwin")

    def test_linux_ripgrep_requires_static_linking(self):
        with patch.object(runtime.subprocess, "check_output", return_value="ripgrep fixture"), patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "not a dynamic executable")):
            runtime.check_ripgrep(Path("rg"), "linux")
        with patch.object(runtime.subprocess, "check_output", return_value="ripgrep fixture"), patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "libpcre2.so => /usr/lib/libpcre2.so (0x123)", "")):
            with self.assertRaisesRegex(ValueError, "static executable"):
                runtime.check_ripgrep(Path("rg"), "linux")

    def test_runtime_archives_reject_links_and_traversal(self):
        for name, kind in [("../escape", tarfile.REGTYPE), ("link", tarfile.SYMTYPE)]:
            archive = self.root / "unsafe.tar.gz"
            with tarfile.open(archive, "w:gz") as output:
                entry = tarfile.TarInfo(name)
                entry.type = kind
                entry.linkname = "/outside"
                output.addfile(entry)
            with self.assertRaisesRegex(ValueError, "unsafe"):
                runtime.extract_regular_archive(archive, self.root)

    def test_linux_dependency_closure_keeps_host_glibc_but_requires_other_libraries(self):
        output = "libcap.so.2 => /usr/lib/libcap.so.2 (0x123)\nlibc.so.6 => /lib/libc.so.6 (0x456)\n"
        with patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, output, "")):
            self.assertEqual(runtime.libraries(Path("bwrap")), [("libcap.so.2", Path("/usr/lib/libcap.so.2"))])
        with patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "libcap.so.2 => not found", "")):
            with self.assertRaisesRegex(ValueError, "Unresolved"):
                runtime.libraries(Path("bwrap"))

    def test_sandbox_requires_all_tools_licenses_and_relocatable_libraries(self):
        source = self.root / "source"
        (source / "bin").mkdir(parents=True)
        (source / "licenses").mkdir()
        (source / "licenses/COPYING").write_text("licenses")
        for name in runtime.SANDBOX_TOOLS:
            (source / "bin" / name).write_bytes(b"\x7fELFfixture")
        destination = self.root / "output with spaces"
        with patch.object(runtime, "libraries", return_value=[]), patch.object(runtime.subprocess, "run") as run, patch.object(runtime.subprocess, "check_output", return_value="bubblewrap 0.9.0\n"):
            info = runtime.prepare_linux_sandbox(destination, source)
        self.assertEqual(len(run.call_args_list), len(runtime.SANDBOX_TOOLS))
        self.assertEqual(info["host_abi"], "glibc")
        wrapper = (destination / "bin/iptables").read_text()
        self.assertIn('export XTABLES_LIBDIR="$root/lib/xtables"', wrapper)
        self.assertIn('exec "$root/libexec/iptables" "$@"', wrapper)
        self.assertNotIn(str(source), wrapper)
        with patch.object(runtime, "libraries", return_value=[("libcap.so.2", Path("/host/libcap.so.2"))]):
            with self.assertRaisesRegex(ValueError, "escaped private"):
                runtime.prepare_linux_sandbox(self.root / "invalid closure", source)
        (source / "bin/bwrap").unlink()
        with self.assertRaisesRegex(ValueError, "bwrap"):
            runtime.prepare_linux_sandbox(self.root / "missing tool", source)
        shutil.rmtree(source / "licenses")
        with self.assertRaisesRegex(ValueError, "licenses"):
            runtime.prepare_linux_sandbox(self.root / "missing licenses", source)

    def test_library_filename_collisions_are_not_silently_overwritten(self):
        first, second, destination = [self.root / name for name in ("first", "second", "libcap.so.2")]
        first.write_text("one")
        second.write_text("two")
        runtime.copy_library(first, destination)
        with self.assertRaisesRegex(ValueError, "Conflicting"):
            runtime.copy_library(second, destination)
        self.assertEqual(destination.read_text(), "one")

    def test_dpkg_license_lookup_includes_only_verified_usr_merge_aliases(self):
        for same_file in (True, False):
            with patch.object(runtime.shutil, "which", side_effect=lambda name: name if name == "dpkg-query" else None), patch.object(Path, "is_file", return_value=True), patch.object(Path, "samefile", return_value=same_file), patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")) as run:
                with self.assertRaisesRegex(ValueError, "license metadata"):
                    runtime.collect_license(Path("/usr/bin/ip"), self.root / "licenses")
            expected = ["dpkg-query", "-S", "/usr/bin/ip"]
            if same_file:
                expected.append("/bin/ip")
            self.assertEqual(run.call_args.args[0], expected)


if __name__ == "__main__":
    unittest.main()
