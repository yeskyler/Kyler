"""Isolated local-installer tests: python3 scripts/tests/test_install.py."""
import os
import json
import hashlib
import shutil
import shlex
import socket
import queue
import threading
import time
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
import tarfile
import sys
import urllib.request

WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
INSTALLER = WORKSPACE_ROOT / "scripts/install.sh"
KYLER_BINARY = WORKSPACE_ROOT / "kyler-rs/target/debug/kyler"


WEB_FIXTURE = '''const http = require('node:http');
const server = http.createServer((req, res) => {
  if (req.url === '/api/service/identity') {
    if (req.headers.authorization !== 'Bearer ' + process.env.KYLER_WEB_SERVICE_SECRET) {
      res.writeHead(401); return res.end();
    }
    res.setHeader('Content-Type', 'application/json');
    return res.end(JSON.stringify({ instance_id: process.env.KYLER_WEB_INSTANCE_ID, pid: process.pid }));
  }
  res.end('bundle fixture');
});
server.listen(Number(process.env.PORT), process.env.HOSTNAME);
process.on('SIGTERM', () => server.close(() => process.exit(0)));
'''


def copy_bundled_runtime(bundle):
    runtime = KYLER_BINARY.parent / "runtime"
    if not (runtime / "tools/rg").is_file():
        raise RuntimeError("Prepare bundled tools first: python3 scripts/prepare_runtime.py --profile debug")
    shutil.copytree(runtime, bundle / "runtime", dirs_exist_ok=True)


def local_bundle(root, binary):
    bundle = Path(tempfile.mkdtemp(prefix="source bundle ", dir=root))
    for folder in ("bin", "runtime", "web"):
        (bundle / folder).mkdir()
    if binary.is_file():
        # Shell fixtures delegate only the installer command to the real CLI.
        # Their version, doctor and lifecycle behavior remains independently fake.
        with binary.open("rb") as stream:
            prefix = stream.read(32)
        if prefix.startswith(b"#!/bin/sh") and b"KYLER_INSTALL_TEST_DELEGATE" not in binary.read_bytes():
            text = binary.read_text()
            first, rest = text.split("\n", 1)
            delegate = '# KYLER_INSTALL_TEST_DELEGATE\ncase " $* " in *" install "*) exec ' + shlex.quote(str(KYLER_BINARY)) + ' "$@" ;; esac\n'
            binary.write_text(first + "\n" + delegate + rest)
        shutil.copy2(binary, bundle / "bin/kyler")
    node = shutil.which("node")
    if not node:
        raise RuntimeError("Node.js is required for installer tests")
    runtime = bundle / "runtime/node"
    runtime.write_text("#!/bin/sh\nexec " + shlex.quote(node) + ' "$@"\n')
    runtime.chmod(0o755)
    (bundle / "web/server.js").write_text(WEB_FIXTURE)
    copy_bundled_runtime(bundle)
    BundleTests.manifest(bundle)
    return bundle


def sandbox_fixture(root, environment):
    tools = root / "sandbox-tools"
    tools.mkdir()
    # Never permit these tests to mutate host packages, even on a Linux test runner.
    for manager in ("apt-get", "dnf", "yum", "pacman", "zypper", "brew"):
        script = tools / manager
        script.write_text('#!/bin/sh\necho "Tests cannot install host packages; prepare sandbox dependencies first." >&2\nexit 1\n')
        script.chmod(0o755)
    environment["PATH"] = str(tools) + os.pathsep + environment["PATH"]
    environment["KYLER_INSTALLER_BINARY"] = str(KYLER_BINARY)



class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="kyler installer ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "user home"
        self.home.mkdir()
        self.source = self.root / "source binary"
        self.source.write_text('#!/bin/sh\nprintf "kyler 0.1.0\\n"\n')
        self.source.chmod(0o755)
        self.env = dict(os.environ, HOME=str(self.home), SHELL="/bin/zsh")
        self.env.pop("KYLER_INTERNAL_TOOL_WORKER", None)
        self.env.pop("XDG_CACHE_HOME", None)
        sandbox_fixture(self.root, self.env)

    def install(self, *args, success=True):
        result = subprocess.run(
            ["sh", str(INSTALLER), "--bundle", str(local_bundle(self.root, self.source)), "--no-start", "--no-configure", *args],
            env=self.env, capture_output=True, text=True, timeout=10,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def remote_install(self, mode="server", version=None):
        source = local_bundle(self.root, self.source)
        archive = self.root / "remote.tar.gz"
        if not archive.exists():
            with tarfile.open(archive, "w:gz") as output:
                output.add(source, arcname="kyler-fixture")
        tools = self.root / "download-tools"
        tools.mkdir(exist_ok=True)
        curl = tools / "curl"
        curl.write_text("#!" + sys.executable + "\n" + r'''import os,sys,json,hashlib,platform
from pathlib import Path
args=sys.argv[1:]
url=next(a for a in args if a.startswith("https://"))
with open(os.environ["DOWNLOAD_LOG"],"a") as log: log.write(json.dumps(args)+"\n")
mode=os.environ["DOWNLOAD_MODE"]
mirror=url.startswith("https://www.kyler.work/")
if mode=="fail" or (mode=="fallback" and mirror) or (mode=="manifest-fallback" and mirror and url.endswith("SHA256SUMS")):
    sys.exit(22)
if url.endswith("/latest"):
    print("https://github.com/yeskyler/Kyler/releases/tag/v0.1.0",end="")
    sys.exit(0)
output=Path(args[args.index("-o")+1])
if url.endswith("latest.txt"):
    output.write_text("not a version" if mode=="invalid-latest" else "v0.1.0\n")
elif url.endswith("SHA256SUMS"):
    arch="aarch64" if platform.machine() in ("arm64","aarch64") else "x86_64"
    target="apple-darwin" if platform.system()=="Darwin" else "unknown-linux-gnu"
    name=f"kyler-v0.1.0-{arch}-{target}.tar.gz"
    digest="0"*64 if mode=="corrupt" else hashlib.sha256(Path(os.environ["REMOTE_ARCHIVE"]).read_bytes()).hexdigest()
    manifest=digest+"  "+name+"\n"
    if mode=="bootstrap":
        installer=f"kyler-installer-v0.1.0-{arch}-{target}"
        manifest+=hashlib.sha256(Path(os.environ["REMOTE_INSTALLER"]).read_bytes()).hexdigest()+"  "+installer+"\n"
    output.write_text(manifest)
elif "kyler-installer-" in url:
    output.write_bytes(Path(os.environ["REMOTE_INSTALLER"]).read_bytes())
else:
    output.write_bytes(Path(os.environ["REMOTE_ARCHIVE"]).read_bytes())
    Path(os.environ["DOWNLOADED_NAME"]).write_text(url.rsplit("/",1)[1])
''')
        curl.chmod(0o755)
        log = self.root / "download.log"
        log.unlink(missing_ok=True)
        environment = dict(self.env, PATH=str(tools)+os.pathsep+self.env["PATH"], DOWNLOAD_LOG=str(log), DOWNLOAD_MODE=mode, REMOTE_ARCHIVE=str(archive), DOWNLOADED_NAME=str(self.root / "asset-name"))
        if mode == "bootstrap":
            environment.pop("KYLER_INSTALLER_BINARY", None)
            environment["REMOTE_INSTALLER"] = str(KYLER_BINARY)
        command = ["sh", str(INSTALLER), "--no-start", "--no-configure"]
        if version: command += ["--version", version]
        result = subprocess.run(command, cwd=self.root, env=environment, capture_output=True, text=True, timeout=20)
        requests = [json.loads(line) for line in log.read_text().splitlines()]
        urls = [next(arg for arg in args if arg.startswith("https://")) for args in requests]
        return result, urls, requests

    def test_bootstrap_downloads_verified_installer_then_cli_downloads_bundle(self):
        result, urls, _ = self.remote_install("bootstrap")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any("kyler-installer-" in url for url in urls))
        self.assertEqual(sum(url.endswith("SHA256SUMS") for url in urls), 2)
        self.assertEqual((result.stdout + result.stderr).count("◇  Prepare package"), 1)
        self.assertTrue((self.home / ".local/bin/kyler").exists())

    def test_offline_archive_bootstraps_from_verified_adjacent_installer(self):
        import platform
        bundle = local_bundle(self.root, self.source)
        target = ("aarch64" if platform.machine() in ("aarch64", "arm64") else "x86_64") + ("-apple-darwin" if platform.system()=="Darwin" else "-unknown-linux-gnu")
        archive = self.root / f"kyler-v0.1.0-{target}.tar.gz"
        with tarfile.open(archive, "w:gz") as stream:
            stream.add(bundle, arcname=archive.name[:-7])
        installer = self.root / f"kyler-installer-v0.1.0-{target}"
        shutil.copy2(KYLER_BINARY, installer)
        (self.root / "SHA256SUMS").write_text(hashlib.sha256(installer.read_bytes()).hexdigest()+"  "+installer.name+"\n")
        environment = self.env.copy()
        environment.pop("KYLER_INSTALLER_BINARY", None)
        command = ["sh", str(INSTALLER), "--bundle", str(archive), "--no-start", "--no-configure"]
        result = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((result.stdout+result.stderr).count("◇  Prepare package"), 1)
        before = (self.home / ".kyler/config.toml").read_bytes()
        installer.write_bytes(b"invalid installer")
        result = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=20)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Installer checksum mismatch", result.stderr)
        self.assertEqual((self.home / ".kyler/config.toml").read_bytes(), before)
        installer.unlink()
        (self.root / "SHA256SUMS").unlink()
        result = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.home / ".kyler/config.toml").read_bytes(), before)

    @unittest.skipUnless(os.name == "posix", "requires Unix signals")
    def test_cli_download_cancellation_leaves_installation_unchanged(self):
        import signal
        marker = self.root / "download-started"
        curl = self.root / "sandbox-tools/curl"
        curl.write_text("#!/bin/sh\n: > " + shlex.quote(str(marker)) + "\nexec sleep 60\n")
        curl.chmod(0o755)
        process = subprocess.Popen([str(KYLER_BINARY), "install", "--version", "0.1.0", "--non-interactive"], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 10
            while not marker.exists() and process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(marker.exists())
            process.send_signal(signal.SIGINT)
            _, stderr = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 130, stderr)
            self.assertIn("Installation cancelled", stderr)
            self.assertFalse((self.home / ".local/bin/kyler").exists())
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_server_download_is_preferred_with_progress_and_all_steps(self):
        result, urls, requests = self.remote_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(all(url.startswith("https://www.kyler.work/releases/") for url in urls))
        self.assertEqual(len(urls), 3)
        self.assertIn("--silent", requests[1])
        output = result.stdout + result.stderr
        for label in ("Prepare package", "Check environment", "Install Kyler"):
            self.assertIn("◇  " + label, output)
        self.assertNotIn("Download order:", output)
        self.assertNotIn("Platform:", output)
        self.assertNotIn("sh install.sh --bundle", output)
        self.assertNotIn("Installation complete", result.stdout)

    @unittest.skipUnless(KYLER_BINARY.is_file(), "build kyler-cli first")
    def test_bundled_rg_is_used_by_installer_and_sandbox_with_broken_host_rg(self):
        bundle = local_bundle(self.root, KYLER_BINARY)
        tool = bundle / "runtime/tools/rg"
        host = self.root / "sandbox-tools/rg"
        host.write_text('#!/bin/sh\n: > "$HOME/host-rg-used"\nexit 77\n')
        host.chmod(0o755)
        BundleTests.manifest(bundle)
        result = subprocess.run([str(KYLER_BINARY), "install", "--bundle", str(bundle), "--no-configure", "--no-start"], env=self.env, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("(bundled)", (self.home / ".kyler/logs/install.log").read_text())
        self.assertIn("◇  Check environment", result.stdout + result.stderr)
        installed = self.home / ".local/bin/kyler"
        doctor = subprocess.run([str(installed), "--json", "doctor"], env=self.env, capture_output=True, text=True, timeout=30)
        self.assertEqual(doctor.returncode, 0, doctor.stderr)
        self.assertTrue(all(check["ripgrep"] for check in json.loads(doctor.stdout)["checks"]))
        self.assertFalse((self.home / "host-rg-used").exists())

    def test_cli_installs_without_bootstrap_and_rejects_modified_bundles(self):
        bundle = local_bundle(self.root, self.source)
        command = [str(KYLER_BINARY), "install", "--bundle", str(bundle), "--no-start", "--no-configure"]
        result = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("◇  Install Kyler", result.stdout + result.stderr)
        self.assertNotIn("[2/2]", result.stdout)
        config = self.home / ".kyler/config.toml"
        binary = self.home / ".local/bin/kyler"
        before = (config.read_bytes(), binary.resolve())
        (bundle / "web/server.js").write_text("unverified")
        failed = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=30)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("checksum mismatch", failed.stderr)
        self.assertEqual(before, (config.read_bytes(), binary.resolve()))

    @unittest.skipUnless(os.name == "posix", "requires Unix file locks")
    def test_cli_install_lock_prevents_concurrent_activation(self):
        import fcntl
        bundle = local_bundle(self.root, self.source)
        lock_path = self.home / ".local/share/kyler/install.lock"
        lock_path.parent.mkdir(parents=True)
        with lock_path.open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = subprocess.run([str(KYLER_BINARY), "install", "--bundle", str(bundle), "--no-start", "--no-configure"], env=self.env, capture_output=True, text=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Another installation", result.stderr)
        self.assertFalse((self.home / ".kyler/config.toml").exists())
        self.assertFalse((self.home / ".local/bin/kyler").exists())

    @unittest.skipUnless(os.name == "posix", "requires Unix signals")
    def test_termination_during_activation_restores_entry_config_and_shell_content(self):
        import signal
        gate = self.root / "startup-gate"
        os.mkfifo(gate)
        marker = self.root / "service-starting"
        self.source.write_text('#!/bin/sh\ncase "$*" in\n*--version*) echo "kyler 0.1.0" ;;\n*doctor*|*service*stop*) exit 0 ;;\n*service*start*) : > '+shlex.quote(str(marker))+'; read line < '+shlex.quote(str(gate))+' ;;\n*) exit 1 ;;\nesac\n')
        bundle = local_bundle(self.root, self.source)
        startup = self.home / ".zshrc"
        startup.write_text("# user shell content\n")
        process = subprocess.Popen([str(KYLER_BINARY), "install", "--bundle", str(bundle), "--no-configure", "--start"], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic()+20
            while not marker.exists() and process.poll() is None and time.monotonic()<deadline:
                time.sleep(0.02)
            self.assertTrue(marker.exists(), "Installer never activated the service")
            process.send_signal(signal.SIGTERM)
            stdout, stderr = process.communicate(timeout=20)
            self.assertEqual(process.returncode, 143, stderr)
            self.assertIn("◇  Restore previous installation", stderr)
            self.assertEqual(startup.read_text(), "# user shell content\n")
            self.assertFalse((self.home / ".local/bin/kyler").is_symlink())
            self.assertFalse((self.home / ".local/share/kyler/current").is_symlink())
            self.assertFalse((self.home / ".kyler/config.toml").exists())
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)

    def test_failed_server_lookup_uses_github_and_pins_download_version(self):
        result, urls, _ = self.remote_install("fallback")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("checking GitHub Releases", (self.home / ".kyler/logs/install.log").read_text())
        self.assertIn("switching to GitHub", (self.home / ".kyler/logs/install.log").read_text())
        self.assertTrue(any("github.com/yeskyler/Kyler/releases/download/v0.1.0/" in url for url in urls))

    def test_manifest_failure_fetches_github_checksums_before_downloading(self):
        result, urls, _ = self.remote_install("manifest-fallback", "0.1.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(url.endswith("latest.txt") or url.endswith("/latest") for url in urls))
        self.assertEqual(sum(url.endswith(".tar.gz") for url in urls), 1)
        self.assertIn("github.com", urls[-1])

    def test_checksum_mismatch_stops_without_fallback_or_installation(self):
        result, urls, _ = self.remote_install("corrupt")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Release checksum mismatch", result.stderr)
        self.assertFalse(any(url.startswith("https://github.com/") for url in urls))
        self.assertFalse((self.home / ".local/bin/kyler").exists())

    def test_both_sources_fail_with_manual_installation_instructions(self):
        result, urls, _ = self.remote_install("fail", "0.1.0")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Both download sources failed", result.stderr)
        self.assertNotIn("Manual installation", result.stderr)
        self.assertIn("kyler install --bundle", (self.home / ".kyler/logs/install.log").read_text())
        self.assertFalse((self.home / ".local/bin/kyler").exists())

    def test_invalid_server_latest_falls_back_to_github_lookup(self):
        result, urls, _ = self.remote_install("invalid-latest")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any(url.endswith("/latest") for url in urls))

    def test_second_install_reuses_verified_cache_without_archive_download(self):
        first, _, _ = self.remote_install()
        self.assertEqual(first.returncode, 0, first.stderr)
        second, urls, _ = self.remote_install()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertFalse(any(url.endswith(".tar.gz") for url in urls))
        self.assertIn("Reusing verified bundle", (self.home / ".kyler/logs/install.log").read_text())

    def test_matching_archive_in_current_directory_is_reused(self):
        first, _, _ = self.remote_install()
        self.assertEqual(first.returncode, 0, first.stderr)
        archive = next((self.home / ".cache/kyler/downloads/v0.1.0").glob("*.tar.gz"))
        shutil.copy2(archive, self.root / archive.name)
        archive.unlink()
        second, urls, _ = self.remote_install()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertFalse(any(url.endswith(".tar.gz") for url in urls))
        self.assertIn("Reusing verified bundle", (self.home / ".kyler/logs/install.log").read_text())

    def test_corrupt_cached_archive_is_replaced(self):
        first, _, _ = self.remote_install()
        self.assertEqual(first.returncode, 0, first.stderr)
        archive = next((self.home / ".cache/kyler/downloads/v0.1.0").glob("*.tar.gz"))
        archive.write_bytes(b"corrupt")
        second, urls, _ = self.remote_install()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertTrue(any(url.endswith(".tar.gz") for url in urls))
        self.assertIn("fresh download is required", (self.home / ".kyler/logs/install.log").read_text())

    def test_first_install_creates_private_config_and_configures_path(self):
        result = self.install()
        binary = self.home / ".local/bin/kyler"
        config = self.home / ".kyler/config.toml"
        self.assertEqual(binary.read_bytes(), self.source.read_bytes())
        self.assertTrue(os.access(binary, os.X_OK))
        self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(config.parent.stat().st_mode), 0o700)
        self.assertIn("enabled = true", config.read_text())
        values = config.read_text()
        self.assertIn('bind = "127.0.0.1:30001"', values)
        self.assertIn('bind = "127.0.0.1:30002"', values)
        self.assertIn('server_url = "http://127.0.0.1:30001"', values)
        self.assertNotIn("api_key =", "\n".join(line for line in config.read_text().splitlines() if not line.startswith("#")))
        self.assertTrue((self.home / ".zshrc").is_file())
        self.assertFalse((self.home / ".kyler/sessions").exists())
        self.assertNotIn('export PATH="$HOME/.local/bin:$PATH"', result.stdout + result.stderr)
        self.assertFalse(list(binary.parent.glob(".kyler-install.*")))
        self.assertFalse(list(config.parent.glob(".config-install.*")))

    def test_upgrade_preserves_config_credentials_sessions_and_adds_path_once(self):
        self.install()
        config = self.home / ".kyler/config.toml"
        config.write_text("# custom config\n[llm]\napi_key='PRIVATE_TEST_KEY'\n")
        secret = self.home / ".kyler/.env"
        secret.write_text("PRIVATE_TEST_ENV=value\n")
        session = self.home / ".kyler/sessions/session/rollout.jsonl"
        session.parent.mkdir(parents=True)
        session.write_text("history\n")
        before = [path.read_bytes() for path in (config, secret, session)]
        self.source.write_text('#!/bin/sh\nprintf "kyler 0.2.0\\n"\n')
        result = self.install()
        self.assertEqual(before, [path.read_bytes() for path in (config, secret, session)])
        self.assertNotIn("PRIVATE_TEST_KEY", result.stdout + result.stderr)
        self.assertEqual((self.home / ".local/bin/kyler").read_bytes(), self.source.read_bytes())
        startup = self.home / ".zshrc"
        self.assertEqual(startup.read_text().count('export PATH="$HOME/.local/bin:$PATH"'), 1)
        # The emitted line must also work with HOME containing spaces.
        output = subprocess.check_output(["sh", "-c", '. "$HOME/.zshrc"; command -v kyler'], env=self.env, text=True)
        self.assertEqual(output.strip(), str(self.home / ".local/bin/kyler"))

    def test_broken_binary_does_not_replace_installed_version(self):
        self.install()
        installed = self.home / ".local/bin/kyler"
        before = installed.read_bytes()
        self.source.write_text("#!/bin/sh\nexit 42\n")
        self.install(success=False)
        self.assertEqual(installed.read_bytes(), before)
        self.assertFalse(list(installed.parent.glob(".kyler-install.*")))
        self.source.write_text("#!/bin/sh\necho wrong-program\n")
        self.install(success=False)
        self.assertEqual(installed.read_bytes(), before)

    def test_existing_binary_can_be_reinstalled_from_itself(self):
        self.install()
        self.source = self.home / ".local/bin/kyler"
        self.env["KYLER_INTERNAL_TOOL_WORKER"] = "1"
        self.install()
        self.assertTrue(self.source.exists())

    def test_missing_source_and_invalid_arguments_do_not_install(self):
        self.source.unlink()
        self.install(success=False)
        self.assertFalse((self.home / ".local").exists())
        self.source.write_text("unused")
        self.install("--unknown", success=False)
        self.assertFalse((self.home / ".local").exists())
        result = subprocess.run(["sh", str(INSTALLER), "--bundle"], env=self.env, capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_help_is_concise_and_removed_options_are_rejected(self):
        result = subprocess.run(["sh", str(INSTALLER), "--help"], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        for option in ("--bundle", "--version", "--help"):
            self.assertIn(option, result.stdout)
        for option in ("--local", "--modify-path", "--no-start", "--no-configure", "--non-interactive"):
            self.assertNotIn(option, result.stdout)
        for option in ("--local", "--modify-path"):
            result = subprocess.run(["sh", str(INSTALLER), option], env=self.env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Unknown argument", result.stderr)
        self.assertFalse((self.home / ".local").exists())

    def test_config_directory_collision_fails_before_replacing_binary(self):
        self.install()
        installed = self.home / ".local/bin/kyler"
        before = installed.read_bytes()
        config = self.home / ".kyler/config.toml"
        config.unlink()
        config.mkdir()
        self.source.write_text('#!/bin/sh\nprintf "kyler 0.2.0\\n"\n')
        self.install(success=False)
        self.assertEqual(installed.read_bytes(), before)

    @unittest.skipUnless((KYLER_BINARY).is_file(), "build kyler-cli first for binary integration")
    def test_real_binary_installs_and_starts_with_generated_configuration(self):
        self.source = KYLER_BINARY
        self.install()
        binary = self.home / ".local/bin/kyler"
        version = subprocess.check_output([str(binary), "--version"], env=self.env, text=True)
        self.assertTrue(version.startswith("kyler "))
        environment = self.env.copy()
        for key in ("KYLER_CONFIG", "KYLER_SESSION_DIR", "KYLER_SESSION_ENABLED", "KYLER_GATEWAY_TOKEN", "KYLER_INTERNAL_TOOL_WORKER"):
            environment.pop(key, None)
        environment["KYLER_SERVER_BIND"] = "127.0.0.1:0"
        environment["KYLER_WEB_ENABLED"] = "false"
        provision = subprocess.run(
            [str(binary), "token", "create", "--name", "installer-test"],
            cwd=self.root, env=environment, capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(provision.returncode, 0, provision.stderr)
        process = subprocess.Popen(
            [str(binary), "serve", "--model", "installer-test", "--base-url", "http://127.0.0.1:1/v1", "--api-key", "installer-test"],
            cwd=self.root, env=environment, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
        )
        lines = queue.Queue()

        def collect():
            for line in process.stderr:
                lines.put(line)
            lines.put(None)

        reader = threading.Thread(target=collect, daemon=True)
        reader.start()
        diagnostics = []
        try:
            deadline = time.monotonic() + 10
            while True:
                line = lines.get(timeout=max(0.1, deadline - time.monotonic()))
                self.assertIsNotNone(line, "".join(diagnostics))
                diagnostics.append(line)
                if line.startswith("Kyler server listening on "):
                    break
                self.assertLess(time.monotonic(), deadline, "".join(diagnostics))
            self.assertTrue((self.home / ".kyler/sessions").is_dir())
            # Wait for the HTTP loop (and its shutdown handler), not just its log line.
            address = line.strip()[len("Kyler server listening on "): ]
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            with opener.open("http://" + address + "/healthz", timeout=5) as response:
                self.assertEqual(response.status, 200)
            process.terminate()
            self.assertEqual(process.wait(timeout=10), 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            reader.join(timeout=2)
            process.stderr.close()

    @unittest.skipUnless((KYLER_BINARY).is_file(), "build kyler-cli first")
    def test_explicit_configure_and_start_preserve_automation(self):
        binary = KYLER_BINARY
        installed = self.home / ".local/bin/kyler"
        environment = self.env.copy()
        for key in list(environment):
            if key.startswith("KYLER_") or key == "OPENAI_API_KEY":
                environment.pop(key)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        environment.update(KYLER_LLM_MODEL="installer-model", KYLER_LLM_BASE_URL="http://127.0.0.1:1/v1", KYLER_LLM_API_KEY="INSTALLER_TEST_KEY", KYLER_SERVER_BIND="127.0.0.1:" + str(port))
        command = ["sh", str(INSTALLER), "--bundle", str(local_bundle(self.root, binary)), "--non-interactive", "--configure", "--start"]
        environment["KYLER_WEB_BIND"] = BundleTests.free_address()
        try:
            first_instance = None
            for _ in range(2):
                result = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("INSTALLER_TEST_KEY", result.stdout + result.stderr)
                record = json.loads((self.home / ".kyler/server.json").read_text())
                self.assertNotIn("workspace",record)
                if first_instance:
                    self.assertNotEqual(record["instance_id"], first_instance)
                first_instance = record["instance_id"]
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                saved = (self.home / ".kyler/config.toml").read_text()
                server = saved.split("[server]\n", 1)[1].split("\n[", 1)[0]
                token = json.loads(next(line.split("=", 1)[1].strip()
                                        for line in server.splitlines()
                                        if line.startswith("access_token =")))
                request = urllib.request.Request(record["url"] + "/v1/info",
                                                 headers={"Authorization": "Bearer " + token})
                with opener.open(request, timeout=5) as response:
                    info = json.load(response)
                self.assertEqual(info["instance_id"], first_instance)
                self.assertEqual(info["model"], "installer-model")
                self.assertNotIn("workspace_root", info)
                self.assertNotIn("working_directory", info)
                self.assertEqual(info["default_workspace"], str((self.home / ".kyler/workspace").resolve()))
                self.assertTrue((self.home / ".kyler/workspace").is_dir())
            self.assertIn("INSTALLER_TEST_KEY", (self.home / ".kyler/config.toml").read_text())
            self.assertTrue((self.home / ".kyler/logs/server.log").is_file())
        finally:
            if installed.exists():
                subprocess.run([str(installed), "service", "stop"], env=environment, capture_output=True, timeout=15)

    def interactive_install(self, replies, expected=0, piped=False, redirected=False, bundle=None, access_scope="1", preserve_addresses=False, advanced=False, direct=False, native=False, complete=True, configure_binary=None, after_install=False, keep_config=False, colored=False):
        start_services = bundle is not None
        if bundle is None:
            bundle = local_bundle(self.root, KYLER_BINARY)
        replies = list(replies)
        if not native:
            if not any(marker.startswith(b"[1] Provider") for marker, _ in replies):
                replies.insert(0, (b"[1] Provider [", "Custom（OpenAI Completion）\n".encode()))
            # Old scenarios describe the same choices; drive the new widgets with
            # searchable labels and real keys instead of command-like model numbers.
            converted=[]
            searching=False
            previous_field=None
            previous_text=b""
            for marker, answer in replies:
                field=marker.split(b"] ",1)[-1].split(b" [",1)[0]
                if field.startswith(b"Context window"):
                    field=b"Context window (tokens or auto)"
                text=answer.rstrip(b"\r\n")
                if field in (b"API URL",b"Context window (tokens or auto)") and field==previous_field:
                    answer=b"\x7f"*len(previous_text)+answer
                previous_field=field
                previous_text=text
                if field==b"Provider" and answer.strip():
                    choice={b"1":b"OpenAI",b"2":b"SiliconFlow",b"3":"Custom（OpenAI Completion）".encode()}.get(answer.strip(),answer.strip())
                    answer=choice+b"\r"
                if field==b"Model":
                    value=answer.strip()
                    if value.startswith(b"/"):
                        answer=value[1:]
                        searching=True
                    elif searching and value.isdigit():
                        answer=b"\r"
                        searching=False
                    elif value:
                        converted.append(("◆  Model".encode(),b"Enter model ID manually\r"))
                        field=b"Model ID"
                if answer==b"\x1b":
                    answer=b"\x03"  # Both Escape and Ctrl+C cancel the upstream prompt sequence.
                converted.append(("◆  ".encode()+field,answer))
            replies=converted
        addresses = []
        if expected == 0 or native:
            if not preserve_addresses:
                for _ in range(2):
                    with socket.socket() as listener:
                        listener.bind(("127.0.0.1", 0))
                        addresses.append("127.0.0.1:" + str(listener.getsockname()[1]))
            if complete and expected == 0:
                answer={"1":b"This machine only\r","2":b"Local network\r","":b"\r"}[access_scope]
                replies += [("◆  Access scope".encode(), answer), ("◆  Channels".encode(), b"\r"), ("◆  Finish setup".encode(), b"\r")]

        if after_install and not keep_config:
            import re
            config = self.home / ".kyler/config.toml"
            if config.exists() and re.search(r'^\s*api_key\s*=\s*"[^"\n]+"', config.read_text(), re.M):
                replies.insert(0, ("◆  Configuration".encode(), b"Edit configuration\r"))

        import pty
        import re
        import select
        import termios
        import fcntl
        binary = configure_binary or KYLER_BINARY
        environment = self.env.copy()
        for name in list(environment):
            if name.startswith("KYLER_") or name == "OPENAI_API_KEY":
                environment.pop(name)
        if addresses:
            environment["KYLER_SERVER_BIND"], environment["KYLER_WEB_BIND"] = addresses
        environment["KYLER_INSTALLER_BINARY"] = str(KYLER_BINARY)
        environment["TERM"]="xterm-256color"
        if colored:
            environment.pop("NO_COLOR", None)
        else:
            environment["NO_COLOR"]="1"
        environment.pop("CLICOLOR_FORCE",None)
        environment.pop("FORCE_COLOR",None)
        master, slave = pty.openpty()
        import struct
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        before = termios.tcgetattr(slave)

        def assert_terminal_restored():
            actual = termios.tcgetattr(slave)
            expected_attributes = list(before)
            # macOS may set PENDIN when returning to canonical input; it is kernel queue state.
            pending = getattr(termios, "PENDIN", 0)
            actual[3] &= ~pending
            expected_attributes[3] &= ~pending
            self.assertEqual(actual, expected_attributes)

        command = ["sh", str(INSTALLER), "--bundle", str(bundle), "--configure"]
        if not start_services:
            command += ["--no-start"]
        else:
            command += ["--start"]
        if after_install:
            command = ["sh", str(INSTALLER), "--bundle", str(bundle)]
        if piped:
            command = ["sh", "-c", 'cat "$1" | sh -s -- --bundle "$2" --no-start --configure', "installer-pipe", str(INSTALLER), str(bundle)]
        elif redirected:
            command = ["sh", "-c", 'sh "$1" --bundle "$2" --no-start --configure </dev/tty', "installer-tty", str(INSTALLER), str(bundle)]

        if advanced or direct:
            command = [str(binary), "--config", str(self.home / ".kyler/config.toml"), "configure", "--addresses"]
            if advanced:
                command += ["--advanced"]

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        command=["sh", "-c", 'echo "__WIZARD_CHILD_PID__:$$"; exec "$@"', "wizard-child", *command]
        # Keep the session leader alive until terminal state has been checked. A
        # pipe gate avoids canonical-mode input races and macOS PTY hangup resets.
        release_read,release_write=os.pipe()
        command=["sh", "-c", 'gate=$1; shift; "$@"; code=$?; echo "__WIZARD_EXIT__:$code"; read release <&"$gate"; exit "$code"', "wizard-terminal", str(release_read), *command]
        process = subprocess.Popen(
            command,
            cwd=self.root, env=environment, stdin=slave, stdout=slave, stderr=slave,
            preexec_fn=controlling_terminal, pass_fds=(release_read,),
        )
        os.close(release_read)
        output = b""
        cursor = 0
        try:
            for marker, answer in replies:
                deadline = time.monotonic() + 15
                def prompt_view():
                    plain = re.sub(rb"\x1b\[[0-9;]*m", b"", output[cursor:])
                    return plain.replace("▲  ".encode(), "◆  ".encode())
                while marker not in prompt_view():
                    if process.poll() is not None or time.monotonic() > deadline:
                        Path("/tmp/kyler-inline-last-terminal.log").write_bytes(output)
                        self.fail("Installer did not show prompt " + repr(marker))
                    if select.select([master], [], [], 0.1)[0]:
                        output += os.read(master, 65536)
                cursor = len(output)
                if answer == b"__SIGNAL_INTERRUPT__":
                    import re, signal
                    child=int(re.search(rb"__WIZARD_CHILD_PID__:(\d+)",output).group(1))
                    os.kill(child,signal.SIGINT)
                else:
                    os.write(master, answer)
                # Widgets redraw on every key. Let the submitted input settle before
                # matching a repeated label, so a prior repaint cannot trigger the next
                # answer while the previous Enter is still being processed.
                settle=time.monotonic()+1
                while time.monotonic()<settle and select.select([master],[],[],0.04)[0]:
                    output+=os.read(master,65536)
            deadline = time.monotonic() + 15
            while b"__WIZARD_EXIT__:" not in output and time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    output += os.read(master, 65536)
            self.assertIn(b"__WIZARD_EXIT__:", output, "Wizard did not finish")
            assert_terminal_restored()
            os.write(release_write,b"\n")
            self.assertEqual(process.wait(timeout=5), expected)
            return output
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(release_write)
            os.close(master)
            os.close(slave)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY")
    def test_colored_setup_uses_foreground_only_and_hides_credentials(self):
        import re
        output = self.interactive_install([
            (b"[2] API URL", b"http://127.0.0.1:1/v1\r"),
            (b"[3] API key", b"COLOR_TEST_KEY\r"),
            (b"[4] Model", b"fixture-model\r"),
        ], colored=True)
        self.assertNotIn(b"COLOR_TEST_KEY", output)
        styles = re.findall(rb"\x1b\[([0-9;]*)m", output)
        self.assertTrue(styles)
        for style in styles:
            codes = [int(code) for code in style.split(b";") if code]
            self.assertFalse(any(40 <= code <= 48 or 100 <= code <= 107 for code in codes), style)
        self.assertNotIn(b"\x1b[?1049h", output)
        initial = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", output.split(b"\n", 1)[1])
        self.assertFalse(initial.startswith((b"\n", b"\r\n")))

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_interactive_install_hides_key_and_restores_terminal(self):
        output = self.interactive_install([
            (b"[2] API URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[3] API key", b"PTY_SECRET_TEST_KEY\r"),
            (b"[4] Model [", b"pty-model\n"),
        ])
        self.assertNotIn(b"PTY_SECRET_TEST_KEY", output)
        self.assertNotIn(b"\x1b[?1049h", output)
        self.assertNotIn(b"\x1b[2J", output)
        self.assertNotIn(b"[1/3]", output)
        config = (self.home / ".kyler/config.toml").read_text()
        self.assertIn("PTY_SECRET_TEST_KEY", config)
        self.assertNotIn("tools.tavily", config)
        for marker in [b"API URL", b"API key", b"Model", b"Configure Kyler"]:
            self.assertIn(marker, output)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_setup_configures_a_channel_and_hides_its_token(self):
        secret = b"123:CHANNEL_TEST_SECRET"
        output = self.interactive_install([
            ("◆  Provider".encode(), "Custom（OpenAI Completion）\r".encode()),
            ("◆  API URL".encode(), b"http://127.0.0.1:1/v1\r"),
            ("◆  API key".encode(), b"MODEL_TEST_KEY\r"),
            ("◆  Model".encode(), b"Enter model ID manually\r"),
            ("◆  Model ID".encode(), b"fixture-model\r"),
            ("◆  Access scope".encode(), b"This machine only\r"),
            ("◆  Channels".encode(), b"Configure channels\r"),
            ("◆  Configure a channel".encode(), b"Telegram\r"),
            ("◆  Bot Token".encode(), secret + b"\r"),
            ("◆  Kyler Web origin".encode(), b"\r"),
            ("◆  Configure a channel".encode(), b"Continue setup\r"),
            ("◆  Finish setup".encode(), b"Edit model\r"),
            ("◆  Model".encode(), b"\r"),
            ("◆  Access scope".encode(), b"\r"),
            ("◆  Channels".encode(), b"Keep current channels\r"),
            ("◆  Finish setup".encode(), b"\r"),
        ], native=True, direct=True, complete=False)
        config = (self.home / ".kyler/config.toml").read_bytes()
        self.assertIn(b"[channels]", config)
        self.assertIn(b'KYLER_TELEGRAM_BOT_TOKEN = "' + secret + b'"', config)
        self.assertNotIn(secret, output)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_cancelled_first_install_does_not_publish_default_configuration(self):
        self.interactive_install([
            ("◆  Provider".encode(), "Custom（OpenAI Completion）\r".encode()),
            ("◆  API URL".encode(), b"http://127.0.0.1:1/v1\r"),
            ("◆  API key".encode(), b"\x03"),
        ], native=True, complete=False, expected=130)
        self.assertFalse((self.home / ".kyler/config.toml").exists())
        self.assertFalse((self.home / ".local/bin/kyler").exists())

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_official_providers_skip_url(self):
        self.install()
        config = self.home / ".kyler/config.toml"
        for label, adapter, url in [
            (b"OpenAI", "openai", "https://api.openai.com/v1"),
            (b"SiliconFlow", "siliconflow", "https://api.siliconflow.cn/v1"),
        ]:
            with self.subTest(provider=adapter):
                config.write_text('[llm]\nadapter="generic"\ndiscover_models=false\nmodel="fixture"\nbase_url="http://127.0.0.1:1/v1"\napi_key="OFFICIAL_FIXTURE_KEY"\n')
                output = self.interactive_install([
                    ("◆  Provider".encode(), label+b"\r"),
                    ("◆  API key".encode(), b"OFFICIAL_FIXTURE_KEY\r"),
                    ("◆  Model".encode(), b"Enter model ID manually\r"),
                    ("◆  Model ID".encode(), b"fixture\r"),
                    ("◆  Access scope".encode(), b"\r"),
                    ("◆  Channels".encode(), b"\r"),
                    ("◆  Finish setup".encode(), b"\r"),
                ], native=True, direct=True, complete=False)
                self.assertNotIn("◆  API URL".encode(), output)
                self.assertNotIn(b"Auto-detect provider", output)
                self.assertNotIn(b"OFFICIAL_FIXTURE_KEY", output)
                saved=config.read_text()
                self.assertIn('adapter = "'+adapter+'"', saved)
                self.assertIn('base_url = "'+url+'"', saved)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_inline_back_navigation_and_review_save(self):
        self.install()
        config = self.home / ".kyler/config.toml"
        config.write_text('[llm]\nadapter="generic"\ndiscover_models=false\nmodel="original"\nbase_url="http://127.0.0.1:1/v1"\napi_key="KEPT_WIZARD_KEY"\n[context]\ncontext_tokens=64000\n')
        output = self.interactive_install([
            ("◆  Provider".encode(), b"\r"),
            ("◆  API URL".encode(), b"http://127.0.0.1:3/v1\r"),
            ("◆  API key".encode(), b"KEPT_WIZARD_KEY\r"),
            ("◆  Model".encode(), b"Back\r"),
            ("◆  API key".encode(), b"\r"),
            ("◆  Model".encode(), b"Enter model ID manually\r"),
            ("◆  Model ID".encode(), b"changed-model\r"),
            ("◆  Access scope".encode(), b"\x1b[B\r"),
            ("◆  Channels".encode(), b"\r"),
            ("◆  Finish setup".encode(), b"Back\r"),
            ("◆  Channels".encode(), b"Back\r"),
            ("◆  Access scope".encode(), b"\x1b[A\r"),
            ("◆  Channels".encode(), b"\r"),
            ("◆  Finish setup".encode(), b"\r"),
        ], native=True, direct=True, complete=False)
        saved=config.read_text()
        self.assertIn('model = "changed-model"', saved)
        self.assertIn('base_url = "http://127.0.0.1:3/v1"', saved)
        self.assertIn('context_tokens = 64000', saved)
        self.assertNotIn('bind = "0.0.0.0:', saved)
        self.assertNotIn(b"KEPT_WIZARD_KEY", output)
        self.assertNotIn(b"\x1b[?1049h", output)
        self.assertNotIn(b"\x1b[2J", output)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_inline_review_cancellation_preserves_configuration(self):
        self.install()
        config=self.home / ".kyler/config.toml"
        config.write_text('[llm]\nadapter="generic"\ndiscover_models=false\nmodel="original"\nbase_url="http://127.0.0.1:1/v1"\napi_key="CANCEL_WIZARD_KEY"\n')
        before=config.read_bytes()
        output=self.interactive_install([
            ("◆  Provider".encode(),b"\r"),
            ("◆  API URL".encode(),b"\r"),
            ("◆  API key".encode(),b"\r"),
            ("◆  Model".encode(),b"\r"),
            ("◆  Access scope".encode(),b"\r"),
            ("◆  Channels".encode(), b"\r"),
            ("◆  Finish setup".encode(),b"\x03"),
        ], native=True, direct=True, complete=False, expected=130)
        self.assertEqual(config.read_bytes(),before)
        self.assertNotIn(b"CANCEL_WIZARD_KEY",output)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_inline_directory_loading_can_be_cancelled_without_saving(self):
        from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
        release=threading.Event()
        class SlowDirectory(BaseHTTPRequestHandler):
            def log_message(self,*args): pass
            def do_GET(self):
                release.wait(timeout=10)
                try:
                    self.send_response(200);self.send_header("Content-Length","11");self.end_headers();self.wfile.write(b'{"data":[]}')
                except OSError: pass
        server=ThreadingHTTPServer(("127.0.0.1",0),SlowDirectory)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        self.install()
        config=self.home / ".kyler/config.toml"
        config.write_text('[llm]\nadapter="generic"\nmodel="original"\nbase_url="http://127.0.0.1:%s/v1"\napi_key="LOADING_WIZARD_KEY"\n' % server.server_port)
        before=config.read_bytes()
        try:
            for interrupt in (b"\x03",b"__SIGNAL_INTERRUPT__"):
                with self.subTest(interrupt=interrupt):
                    output=self.interactive_install([
                        ("◆  Provider".encode(),b"\r"),
                        ("◆  API URL".encode(),b"\r"),
                        ("◆  API key".encode(),b"\r"),
                        ("Fetching model directory".encode(),interrupt),
                    ],native=True,direct=True,complete=False,expected=130)
                    self.assertEqual(config.read_bytes(),before)
                    self.assertNotIn(b"LOADING_WIZARD_KEY",output)
        finally:
            release.set();server.shutdown();server.server_close();thread.join(timeout=2)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_inline_retry_action_can_fetch_from_a_manual_only_connection(self):
        from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
        requests=[]
        class Directory(BaseHTTPRequestHandler):
            def log_message(self,*args): pass
            def do_GET(self):
                requests.append(self.path)
                data=json.dumps({"data":[{"id":"retry-model"}]+[{"id":f"retry-{i}"} for i in range(9)]}).encode()
                self.send_response(200);self.send_header("Content-Length",str(len(data)));self.end_headers();self.wfile.write(data)
        server=ThreadingHTTPServer(("127.0.0.1",0),Directory)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        self.install();config=self.home / ".kyler/config.toml"
        config.write_text('[llm]\nadapter="generic"\ndiscover_models=false\nmodel="original"\nbase_url="http://127.0.0.1:%s/v1"\napi_key="RETRY_WIZARD_KEY"\n' % server.server_port)
        try:
            output=self.interactive_install([
                ("◆  Provider".encode(),b"\r"),
                ("◆  API URL".encode(),b"\r"),
                ("◆  API key".encode(),b"\r"),
                ("◆  Model".encode(),b"Retry discovery\r"),
                ("◆  Model".encode(),b"\x1b[B"*8+b"retry-model"+b"\x1b[A"*20+b"\r"),
            ],native=True,direct=True,colored=True)
            self.assertEqual(requests,["/v1/models"])
            self.assertIn(b"Models: 10 available", output)
            self.assertIn('model = "retry-model"',config.read_text())
            self.assertIn('discover_models = false',config.read_text())
            self.assertNotIn(b"RETRY_WIZARD_KEY",output)
            import re
            plain = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", output)
            self.assertRegex(plain, rb"[0-9]+\. retry-model")
            self.assertNotRegex(plain, rb"(?:^|[\r\n])[ \t]*[\^v] [0-9]+\)")
            self.assertIn("│".encode(), plain)
            self.assertIn("● Custom（OpenAI Completion）".encode(), plain)
        finally:
            server.shutdown();server.server_close();thread.join(timeout=2)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_remote_model_selection_search_and_auto_capacity(self):
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        requests = []
        mode = {"failed": False}
        class Models(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_GET(self):
                requests.append((self.path, self.headers.get("Authorization")))
                if mode["failed"]:
                    self.send_response(503)
                    self.end_headers()
                    return
                data = json.dumps({"data": [{"id": "gpt-5.6-sol"}, {"id": "text-embedding-3-small"}, {"id": "gpt-4.1-mini"}]}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
        server = ThreadingHTTPServer(("127.0.0.1", 0), Models)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = "http://127.0.0.1:%s/v1" % server.server_port
            output = self.interactive_install([
                (b"[1] Provider [", "Custom（OpenAI Completion）\n".encode()),
                (b"[2] API URL [", (url + "\n").encode()),
                (b"[3] API key", b"DISCOVERY_TEST_KEY\r"),
                (b"[4] Model [", b"/5.6\n"),
                (b"[4] Model [", b"1\n"),
            ])
            self.assertEqual(requests, [("/v1/models", "Bearer DISCOVERY_TEST_KEY")])
            self.assertNotIn(b"fallback budget", output.lower())
            self.assertNotIn(b"text-embedding-3-small", output)
            self.assertNotIn(b"DISCOVERY_TEST_KEY", output)
            saved = (self.home / ".kyler/config.toml").read_text()
            self.assertIn('model = "gpt-5.6-sol"', saved)
            self.assertIn('adapter = "generic"', saved)
            self.assertIn("auto_model_window = true", saved)
            self.assertNotIn("context_tokens", saved)
            replies = [
                (b"[2] API URL [", b"\n"),
                (b"[3] API key", b"\r"),
                (b"[4] Model [gpt-5.6-sol]", b"\n"),
            ]
            self.interactive_install(replies, direct=True)
            cache = self.home / ".kyler/sessions/_model-catalog.json"
            self.assertTrue(cache.is_file())
            self.assertNotIn("DISCOVERY_TEST_KEY", cache.read_text())
            mode["failed"] = True
            cached = self.interactive_install(replies, direct=True)
            self.assertIn(b"Models: 2 cached", cached)
            self.assertNotIn(b"fallback budget", cached.lower())
            self.assertNotIn(b"DISCOVERY_TEST_KEY", cached)
            self.assertEqual(len(requests), 3)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_context_window_is_automatic_and_advanced_override_is_optional(self):
        output = self.interactive_install([
            (b"[2] API URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[3] API key", b"WINDOW_TEST_KEY\r"),
            (b"[4] Model [", b"window-model\n"),
        ])
        self.assertNotIn(b"Context window (tokens", output)
        self.assertNotIn(b"fallback budget; not a verified model capacity", output)
        config = self.home / ".kyler/config.toml"
        self.assertIn("auto_model_window = true", config.read_text())
        output = self.interactive_install([
            (b"[2] API URL [", b"\n"),
            (b"[3] API key", b"\r"),
            (b"[4] Model [", b"\n"),
            (b"[5] Context window (tokens or auto)", b"invalid\n"),
            (b"[5] Context window (tokens or auto)", b"8000\n"),
            (b"[5] Context window (tokens or auto)", b"64k\n"),
        ], advanced=True)
        self.assertIn(b"Invalid context window", output)
        self.assertIn("context_tokens = 64000", config.read_text())
        output = self.interactive_install([
            (b"[2] API URL [", b"\n"),
            (b"[3] API key", b"\r"),
            (b"[4] Model [", b"\n"),
        ])
        self.assertIn("context_tokens = 64000", config.read_text())
        output = self.interactive_install([
            (b"[2] API URL [", b"\n"),
            (b"[3] API key", b"\r"),
            (b"[4] Model [", b"\n"),
            (b"[5] Context window (tokens or auto)", b"auto\n"),
        ], advanced=True)
        self.assertIn("auto_model_window = true", config.read_text())
        self.assertIn("context_tokens = 64000", config.read_text())
        self.assertIn(b"fallback budget; not a verified model capacity", output)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_named_profiles_keep_unselected_windows_and_advanced_override(self):
        config = self.home / ".kyler/config.toml"
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text('''[models]
default = "a_small"
[models.providers.fixture]
base_url = "http://127.0.0.1:1/v1"
api_key = "PROFILE_TEST_KEY"
[models.profiles.a_small]
provider = "fixture"
model = "small-model"
context_tokens = 64000
output_tokens = 1000
[models.profiles.b_large]
provider = "fixture"
model = "large-model"
context_tokens = 128000
output_tokens = 8192
''')
        output = self.interactive_install([
            (b"[2] API URL", b"\n"),
            (b"[3] API key", b"\r"),
            (b"[4] Model [small-model]", b"\n"),
            (b"[5] Context window (tokens or auto) [64000]", b"32k\n"),
        ], advanced=True)
        text = config.read_text()
        self.assertIn("context_tokens = 32000", text)
        self.assertIn("context_tokens = 128000", text)
        self.assertIn("output_tokens = 1000", text)
        self.assertIn("output_tokens = 8192", text)
        self.assertNotIn(b"PROFILE_TEST_KEY", output)

    @unittest.skipUnless(os.name == "posix" and KYLER_BINARY.is_file(), "requires PTY and kyler binary")
    def test_lan_scope_generates_tokens_without_address_or_password_prompts(self):
        output = self.interactive_install([
            (b"[2] API URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[3] API key", b"LAN_MODEL_TEST_KEY\r"),
            (b"[4] Model [", b"fixture-model\n"),
        ], access_scope="2")
        text = (self.home / ".kyler/config.toml").read_text()
        self.assertEqual(text.count('bind = "0.0.0.0:'), 2)
        self.assertIn('server_url = "http://127.0.0.1:', text)
        self.assertNotIn(b"Backend address", output)
        self.assertNotIn(b"Web address", output)
        self.assertNotIn(b"Web access password", output)
        self.assertNotIn(b"LAN_MODEL_TEST_KEY", output)
        self.assertNotIn(b"kyler service token", output)
        again = self.interactive_install([
            (b"[2] API URL [", b"\n"),
            (b"[3] API key", b"\r"),
            (b"[4] Model [", b"\n"),
        ], access_scope="", preserve_addresses=True)
        self.assertIn(b"Local network", again)
        self.assertEqual((self.home / ".kyler/config.toml").read_text(), text)

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_piped_installer_reads_hidden_key_from_controlling_terminal(self):
        output = self.interactive_install([
            (b"[2] API URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[3] API key", b"PIPE_SECRET_TEST_KEY\r"),
            (b"[4] Model [", b"pipe-model\n"),
        ], piped=True)
        self.assertNotIn(b"PIPE_SECRET_TEST_KEY", output)
        self.assertIn("PIPE_SECRET_TEST_KEY", (self.home / ".kyler/config.toml").read_text())

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_dev_tty_input_does_not_require_event_reader_initialization(self):
        output = self.interactive_install([
            (b"[2] API URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[3] API key", b"TTY_SECRET_TEST_KEY\r"),
            (b"[4] Model [", b"tty-model\n"),
        ], redirected=True)
        self.assertNotIn(b"TTY_SECRET_TEST_KEY", output)
        self.assertIn("TTY_SECRET_TEST_KEY", (self.home / ".kyler/config.toml").read_text())

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_setup_retries_invalid_input_without_extra_configuration_steps(self):
        output = self.interactive_install([
            (b"[2] API URL [", b"not-a-url\n"),
            (b"[2] API URL [", b"https://example.com/v1/chat/completions\n"),
            (b"[2] API URL [", b"  http://127.0.0.1:1/v1/  \n"),
            (b"[3] API key", b"\r"),
            (b"[3] API key", b"\x1b[200~first\nsecond\x1b[201~\r"),
            (b"[3] API key", b"\x1b[200~  RETRY_SECRET_KEY\n\x1b[201~\r"),
            (b"[4] Model [", b"  pty-model  \n"),
        ])
        self.assertNotIn(b"RETRY_SECRET_KEY", output)
        self.assertNotIn(b"first", output)
        self.assertNotIn(b"second", output)
        config = (self.home / ".kyler/config.toml").read_text()
        self.assertIn('model = "pty-model"', config)
        self.assertIn('base_url = "http://127.0.0.1:1/v1"', config)
        self.assertIn('api_key = "RETRY_SECRET_KEY"', config)
        self.assertNotIn('tools.tavily', config)
        self.assertNotIn(b"Backend URL", output)

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_setup_cancellation_keeps_existing_config_and_binary(self):
        self.install()
        config = self.home / ".kyler/config.toml"
        config.write_text('[llm]\ndiscover_models=false\nmodel="original-model"\nbase_url="https://example.com/v1"\napi_key="ORIGINAL_TEST_KEY"\n')
        before = config.read_bytes()
        installed = self.home / ".local/bin/kyler"
        binary_before = installed.read_bytes()
        output = self.interactive_install([
            (b"[2] API URL [", b"\n"),
            (b"[3] API key", b"\x1b"),
        ], expected=130)
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(installed.read_bytes(), binary_before)
        self.assertNotIn(b"ORIGINAL_TEST_KEY", output)
        self.assertFalse(list(installed.parent.glob(".kyler-install.*")))

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_enter_keeps_existing_values_and_hidden_keys(self):
        self.install()
        config = self.home / ".kyler/config.toml"
        config.write_text('[llm]\ndiscover_models=false\nmodel="kept-model"\nbase_url="https://example.com/v1"\napi_key="KEPT_MODEL_KEY"\n[tools.tavily]\nenabled=true\napi_key="KEPT_SEARCH_KEY"\n[context]\ncontext_tokens=64000\n')
        output = self.interactive_install([
            (b"[2] API URL [", b"\n"),
            (b"[3] API key", b"\r"),
            (b"[4] Model [", b"\n"),
        ])
        self.assertNotIn(b"KEPT_MODEL_KEY", output)
        self.assertNotIn(b"KEPT_SEARCH_KEY", output)
        saved = config.read_text()
        self.assertNotIn("tools.tavily", saved)
        self.assertNotIn("KEPT_SEARCH_KEY", saved)
        for value in ['model = "kept-model"', 'base_url = "https://example.com/v1"', 'api_key = "KEPT_MODEL_KEY"', 'context_tokens = 64000']:
            self.assertIn(value, saved)

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_invalid_saved_key_retries_without_advancing_or_exposing_credentials(self):
        self.install()
        config = self.home / ".kyler/config.toml"
        config.write_text('[llm]\ndiscover_models=false\nmodel="kept-model"\nbase_url="https://example.com/v1"\napi_key="invalid saved key"\n')
        output = self.interactive_install([
            (b"[2] API URL [", b"\n"),
            (b"[3] API key", b"\r"),
            (b"[3] API key", b"REPLACEMENT_TEST_KEY\r"),
            (b"[4] Model [", b"\n"),
        ])
        self.assertIn(b"Saved key is invalid", output)
        self.assertEqual(output.count("◆  API key".encode()), 2)
        self.assertNotIn(b"Backend URL", output)
        self.assertNotIn(b"invalid saved key", output)
        self.assertNotIn(b"REPLACEMENT_TEST_KEY", output)
        self.assertIn('api_key = "REPLACEMENT_TEST_KEY"', config.read_text())

    def test_server_start_failure_shows_one_reason_and_private_log(self):
        self.source.write_text("#!/bin/sh\ncase \"$*\" in\n*--version*) echo 'kyler 0.1.0' ;;\n*service*stop*) exit 0 ;;\n*doctor*) exit 0 ;;\n*configure*) exit 0 ;;\n*) echo 'Server address is in use.' >&2; exit 2 ;;\nesac\n")
        result = subprocess.run(
            ["sh", str(INSTALLER), "--bundle", str(local_bundle(self.root, self.source)), "--no-configure", "--start"],
            env=self.env, capture_output=True, text=True, timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Server address is in use.", result.stderr)
        self.assertIn("Log:", result.stderr)
        self.assertNotIn("kyler service status", result.stdout + result.stderr)
        self.assertIn("Server address is in use.", (self.home / ".kyler/logs/install.log").read_text())

    def test_bash_and_unknown_shell_startup_files(self):
        self.env["SHELL"] = "/bin/bash"
        self.install()
        self.assertTrue((self.home / ".bashrc").is_file())
        self.env["SHELL"] = "/bin/sh"
        self.install()
        self.assertTrue((self.home / ".profile").is_file())


    def test_failed_doctor_preserves_existing_installation_and_configuration(self):
        self.install()
        installed = self.home / ".local/bin/kyler"
        current = self.home / ".local/share/kyler/current"
        config = self.home / ".kyler/config.toml"
        before = (installed.resolve(), current.resolve(), config.read_bytes())
        self.source.write_text('#!/bin/sh\nif [ "$1" = doctor ]; then echo "namespace denied" >&2; exit 1; fi\nprintf "kyler 0.2.0\\n"\n')
        result = self.install(success=False)
        self.assertIn("Sandbox verification failed", result.stderr)
        self.assertEqual(before, (installed.resolve(), current.resolve(), config.read_bytes()))
        self.assertNotIn("◇  Install Kyler", result.stdout + result.stderr)

    def test_failed_doctor_on_first_install_does_not_create_configuration(self):
        self.source.write_text('#!/bin/sh\nif [ "$1" = doctor ]; then exit 1; fi\nprintf "kyler 0.1.0\\n"\n')
        self.install(success=False)
        self.assertFalse((self.home / ".kyler/config.toml").exists())
        self.assertFalse((self.home / ".local/bin/kyler").exists())


@unittest.skipUnless((KYLER_BINARY).is_file() and shutil.which("node"), "requires kyler binary and Node")
class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="kyler bundle ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("KYLER_") and key != "OPENAI_API_KEY"}
        sandbox_fixture(self.root, self.env)
        self.env.update(HOME=str(self.home), KYLER_LLM_MODEL="bundle-test", KYLER_LLM_BASE_URL="http://127.0.0.1:1/v1", KYLER_LLM_API_KEY="BUNDLE_TEST_KEY")
        self.env["KYLER_SERVER_BIND"] = self.free_address()
        self.env["KYLER_WEB_BIND"] = self.free_address()
        self.binary = self.home / ".local/bin/kyler"
        self.addCleanup(self.stop)

    @staticmethod
    def free_address():
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            return "127.0.0.1:" + str(listener.getsockname()[1])

    def stop(self):
        if self.binary.exists():
            subprocess.run([str(self.binary), "service", "stop"], env=self.env, capture_output=True, timeout=50)

    def bundle(self, name, broken=False):
        bundle = self.root / name
        for folder in ("bin", "runtime", "web"):
            (bundle / folder).mkdir(parents=True)
        shutil.copy2(KYLER_BINARY, bundle / "bin/kyler")
        shutil.copy2(shutil.which("node"), bundle / "runtime/node")
        script = WEB_FIXTURE
        (bundle / "web/server.js").write_text("process.exit(42);" if broken else script)
        copy_bundled_runtime(bundle)
        self.manifest(bundle)
        return bundle

    @staticmethod
    def manifest(bundle):
        entries = []
        for path in sorted(bundle.rglob("*")):
            if path.is_file() and path.name != "SHA256SUMS":
                entries.append(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + path.relative_to(bundle).as_posix())
        (bundle / "SHA256SUMS").write_text("\n".join(entries) + "\n")

    def install(self, bundle, success=True, configure=True, start=True):
        command = ["sh", str(INSTALLER), "--bundle", str(bundle), "--non-interactive"]
        if configure: command.append("--configure")
        if start is True: command.append("--start")
        elif start is False: command.append("--no-start")
        result = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=90)
        self.assertEqual(result.returncode == 0, success, f"exit={result.returncode}\n" + result.stdout + result.stderr)
        self.assertNotIn("BUNDLE_TEST_KEY", result.stdout + result.stderr)
        return result

    def status(self, *args):
        result = subprocess.run([str(self.binary), "--json", "service", *args], env=self.env, capture_output=True, text=True, timeout=50)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_default_install_is_quiet_and_requires_separate_configuration(self):
        self.env.pop("KYLER_LLM_API_KEY")
        result = self.install(self.bundle("quiet"), configure=False, start=None)
        output = result.stdout + result.stderr
        self.assertNotIn("Installation complete.", output)
        self.assertNotIn("Kyler Installer", output)
        self.assertIn("configure", output)
        for hidden in ("Platform:", "Web token:", "service start", "ripgrep 15", "\x1b["):
            self.assertNotIn(hidden, output)
        config = self.home / ".kyler/config.toml"
        self.assertTrue(config.exists())
        self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
        self.assertFalse((self.home / ".kyler/server.json").exists())
        self.assertFalse((self.home / ".kyler/web.json").exists())
        self.assertIn("ripgrep", (self.home / ".kyler/logs/install.log").read_text())

    def test_default_upgrade_preserves_configuration_and_running_state(self):
        self.install(self.bundle("upgrade-first"))
        config = self.home / ".kyler/config.toml"
        before = config.read_bytes()
        self.install(self.bundle("upgrade-running"), configure=False, start=None)
        self.assertEqual(config.read_bytes(), before)
        state = self.status("status")
        self.assertTrue(state["backend"]["running"])
        self.assertTrue(state["web"]["running"])
        self.status("stop")
        self.install(self.bundle("upgrade-stopped"), configure=False, start=None)
        self.assertEqual(config.read_bytes(), before)
        self.assertFalse(self.status("status")["running"])

    def test_complete_install_runs_both_services_and_rolls_back_failed_upgrade(self):
        first = self.bundle("first")
        self.install(first)
        self.assertTrue(self.binary.is_symlink())
        current = self.home / ".local/share/kyler/current"
        version = current.resolve()
        state = self.status("status")
        self.assertTrue(state["backend"]["running"])
        self.assertTrue(state["web"]["running"])
        config = self.home / ".kyler/config.toml"
        before = config.read_bytes()
        # Reinstallation is idempotent and preserves configuration.
        self.install(first)
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(current.resolve(), version)
        # A changed address is captured in the candidate, then reverted on failure.
        self.env["KYLER_WEB_BIND"] = self.free_address()
        result = self.install(self.bundle("broken", broken=True), success=False)
        self.assertIn("◇  Restore previous installation", result.stderr)
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(current.resolve(), version)
        self.env.pop("KYLER_WEB_BIND")
        state = self.status("status")
        self.assertTrue(state["running"])
        self.assertEqual(stat.S_IMODE((self.home / ".kyler/web.json").stat().st_mode), 0o600)
        self.assertFalse(self.status("stop")["running"])
        self.assertFalse(self.status("status")["web"]["running"])

    def test_reinstall_replaces_runtime_cache_and_removes_old_versions(self):
        bundle = self.bundle("repair-reinstall")
        self.install(bundle, start=False)
        current = self.home / ".local/share/kyler/current"
        old = current.resolve()
        config = self.home / ".kyler/config.toml"
        before = config.read_bytes()
        cache = old / "web/.next/server/route-cache/generated.html"
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text("runtime cache")
        self.install(bundle, configure=False, start=False)
        replacement = current.resolve()
        self.assertNotEqual(replacement, old)
        self.assertFalse(old.exists())
        self.assertFalse((replacement / cache.relative_to(old)).exists())
        self.assertEqual(config.read_bytes(), before)
        (replacement / "web/server.js").write_text("modified")
        self.install(bundle, configure=False, start=False)
        self.assertNotEqual(current.resolve(), replacement)
        self.assertFalse(replacement.exists())
        self.assertEqual((current.resolve() / "web/server.js").read_text(), WEB_FIXTURE)
        self.assertEqual(config.read_bytes(), before)

    def test_doctor_failure_keeps_running_services_and_configuration(self):
        self.install(self.bundle("doctor-first"))
        before = self.status("status")
        current = (self.home / ".local/share/kyler/current").resolve()
        config = self.home / ".kyler/config.toml"
        contents = config.read_bytes()
        broken = self.bundle("doctor-failure")
        (broken / "bin/kyler").write_text('#!/bin/sh\nif [ "$1" = doctor ]; then echo "sandbox unavailable" >&2; exit 1; fi\nexec ' + shlex.quote(str(KYLER_BINARY)) + ' "$@"\n')
        self.manifest(broken)
        result = self.install(broken, success=False)
        self.assertIn("Sandbox verification failed", result.stderr)
        self.assertEqual((self.home / ".local/share/kyler/current").resolve(), current)
        self.assertEqual(config.read_bytes(), contents)
        after = self.status("status")
        self.assertTrue(after["running"])
        self.assertEqual(before["backend"]["pid"], after["backend"]["pid"])
        self.assertEqual(before["web"]["pid"], after["web"]["pid"])

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_interactive_bundle_uses_three_stages_and_starts_services(self):
        self.env.pop("KYLER_SERVER_BIND")
        self.env.pop("KYLER_WEB_BIND")
        output = InstallTests.interactive_install(self, [
            (b"[2] API URL [https://api.siliconflow.cn/v1]", b"http://127.0.0.1:1/v1\n"),
            (b"[3] API key", b"WIZARD_TEST_KEY\r"),
            (b"[4] Model [deepseek-ai/DeepSeek-V4-Flash]", b"deepseek-ai/DeepSeek-V4-Flash\r"),
        ], bundle=self.bundle("interactive"))
        titles = [b"Channels", b"Review setup", b"Start services"]
        self.assertEqual([output.index(title) for title in titles], sorted(output.index(title) for title in titles))
        self.assertNotIn(b"Local access:", output)
        self.assertIn(b"OpenAI", output)
        self.assertNotIn(b"WIZARD_TEST_KEY", output)
        self.assertNotIn(b"Backend URL", output)
        self.assertNotIn(b"next page", output.lower())
        self.assertNotIn(b"kyler service status", output)
        self.assertNotIn(b"kyler service start --restart", output)
        config = (self.home / ".kyler/config.toml").read_text()
        self.assertIn('model = "deepseek-ai/DeepSeek-V4-Flash"', config)
        self.assertIn('base_url = "http://127.0.0.1:1/v1"', config)
        state = self.status("status")
        self.assertTrue(state["backend"]["running"])
        self.assertTrue(state["web"]["running"])
        self.assertIn('server_url = "' + state["backend"]["url"] + '"', config)

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_installed_configuration_can_start_or_save_only(self):
        self.install(self.bundle("configure-later"), configure=False, start=None)
        self.env.pop("KYLER_SERVER_BIND")
        self.env.pop("KYLER_WEB_BIND")
        config = self.home / ".kyler/config.toml"
        config.write_text(config.read_text().replace("[llm]", "[llm]\ndiscover_models = false", 1))
        output = InstallTests.interactive_install(self, [
            (b"[2] API URL", b"http://127.0.0.1:1/v1\r"),
            (b"[3] API key", b"CONFIGURE_TEST_KEY\r"),
            (b"[4] Model", b"fixture-model\r"),
        ], direct=True, configure_binary=self.binary)
        self.assertIn(b"Save and start", output)
        self.assertNotIn(b"CONFIGURE_TEST_KEY", output)
        self.assertTrue(self.status("status")["running"])
        self.status("stop")
        output = InstallTests.interactive_install(self, [
            ("◆  Provider".encode(), b"\r"),
            ("◆  API URL".encode(), b"\r"),
            ("◆  API key".encode(), b"\r"),
            ("◆  Model".encode(), b"\r"),
            ("◆  Access scope".encode(), b"\r"),
            ("◆  Channels".encode(), b"\r"),
            ("◆  Finish setup".encode(), b"Save without starting\r"),
        ], native=True, complete=False, direct=True, configure_binary=self.binary)
        self.assertIn("◇  Configure Kyler".encode(), output)
        self.assertFalse(self.status("status")["running"])

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_default_interactive_install_configures_as_the_final_step(self):
        self.env.pop("KYLER_SERVER_BIND")
        self.env.pop("KYLER_WEB_BIND")
        output = InstallTests.interactive_install(self, [
            (b"[2] API URL", b"http://127.0.0.1:1/v1\r"),
            (b"[3] API key", b"FINAL_STEP_TEST_KEY\r"),
            (b"[4] Model", b"fixture-model\r"),
        ], bundle=self.bundle("final-configuration"), after_install=True)
        labels = ["◇  Prepare package", "◇  Check environment", "◇  Install Kyler", "◆  Configure Kyler"]
        positions = [output.index(label.encode()) for label in labels]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("◇  Configure Kyler".encode(), output)
        self.assertNotIn(b"Kyler Installer", output)
        self.assertNotIn(b"Installation complete", output)
        self.assertNotIn(b"FINAL_STEP_TEST_KEY", output)
        self.assertTrue(self.status("status")["running"])

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_cancelled_final_configuration_keeps_the_installed_program(self):
        output = InstallTests.interactive_install(self, [
            (b"[2] API URL", b"http://127.0.0.1:1/v1\r"),
            (b"[3] API key", b"\x03"),
        ], bundle=self.bundle("cancel-final-configuration"), after_install=True, complete=False)
        self.assertTrue(self.binary.exists())
        self.assertIn(b"Configure Kyler: skipped", output)
        self.assertIn(b"run kyler configure", output)
        self.assertFalse((self.home / ".kyler/server.json").exists())

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_interactive_reinstall_opens_configuration_with_existing_settings(self):
        bundle = self.bundle("reconfigure-existing")
        self.install(bundle, start=False)
        before = (self.home / ".kyler/config.toml").read_bytes()
        output = InstallTests.interactive_install(self, [
            (b"[2] API URL", b"\r"),
            (b"[3] API key", b"\x03"),
        ], bundle=bundle, after_install=True, complete=False)
        self.assertIn("◆  Configure Kyler".encode(), output)
        self.assertTrue(any(frame.encode() in output for frame in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"))
        self.assertIn(b"Configure Kyler: skipped", output)
        self.assertEqual((self.home / ".kyler/config.toml").read_bytes(), before)

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_service_restoration_happens_after_cancelled_configuration(self):
        bundle = self.bundle("defer-service-restoration")
        self.install(bundle)
        self.env.pop("KYLER_SERVER_BIND")
        self.env.pop("KYLER_WEB_BIND")
        output = InstallTests.interactive_install(self, [
            (b"[2] API URL", b"\r"),
            (b"[3] API key", b"\x03"),
        ], bundle=bundle, after_install=True, complete=False)
        self.assertLess(output.index("◆  Configure Kyler".encode()), output.index("◇  Restore services".encode()))
        self.assertTrue(self.status("status")["running"])

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_upgrade_can_keep_configuration_and_start_without_model_discovery(self):
        bundle = self.bundle("keep-current-setup")
        self.install(bundle, start=False)
        self.env.pop("KYLER_SERVER_BIND")
        self.env.pop("KYLER_WEB_BIND")
        config = self.home / ".kyler/config.toml"
        before = config.read_bytes()
        output = InstallTests.interactive_install(self, [
            ("◆  Configuration".encode(), b"\r"),
        ], native=True, complete=False, keep_config=True, after_install=True,
           preserve_addresses=True, bundle=bundle)
        self.assertEqual(config.read_bytes(), before)
        self.assertNotIn("◆  Provider".encode(), output)
        self.assertNotIn(b"Web token:", output)
        self.assertIn(b"Interface  ", output)
        self.assertRegex(output, rb"Token\s+[A-Za-z0-9_-]+")
        self.assertLess(output.index("◇  Configure Kyler".encode()), output.index("◇  Launch".encode()))
        self.assertTrue(self.status("status")["running"])

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_network_completion_uses_primary_interface_for_wildcard_web_listener(self):
        import re
        port = self.env["KYLER_WEB_BIND"].rsplit(":", 1)[1]
        self.env["KYLER_WEB_BIND"] = "0.0.0.0:" + port
        bundle = self.bundle("network-completion")
        self.install(bundle, start=False)
        self.env.pop("KYLER_SERVER_BIND")
        self.env.pop("KYLER_WEB_BIND")
        output = InstallTests.interactive_install(self, [
            ("◆  Configuration".encode(), b"\r"),
        ], native=True, complete=False, keep_config=True, after_install=True,
           preserve_addresses=True, bundle=bundle)
        interface = re.search(rb"Interface\s+[^\r\n]+ \xc2\xb7 ([0-9.]+)", output)
        if interface:
            self.assertIn(b"Web        http://" + interface.group(1) + b":" + port.encode(), output)
        else:
            self.assertIn(b"Interface  Primary network connection unavailable", output)
        self.assertRegex(output, rb"Token\s+[A-Za-z0-9_-]+")
        self.assertTrue(self.status("status")["web"]["running"])

    def test_corrupt_bundle_is_rejected_before_installation(self):
        bundle = self.bundle("corrupt")
        (bundle / "web/server.js").write_text("modified")
        self.install(bundle, success=False)
        self.assertFalse(self.binary.exists())


class SystemCompatibilityTests(unittest.TestCase):
    def check(self, system="Linux", architecture="x86_64", libc="glibc 2.31"):
        with tempfile.TemporaryDirectory() as directory:
            tools = Path(directory)
            for name, body in {
                "uname": 'if [ "$1" = -s ]; then echo "$TEST_SYSTEM"; else echo "$TEST_ARCH"; fi',
                "getconf": 'printf "%s\\n" "$TEST_LIBC"',
            }.items():
                path = tools / name
                path.write_text("#!/bin/sh\n" + body + "\n")
                path.chmod(0o755)
            installer = tools / "installer"
            installer.write_text("#!/bin/sh\nexit 0\n")
            installer.chmod(0o755)
            env = dict(os.environ, HOME=directory, PATH=str(tools)+os.pathsep+os.environ["PATH"], TEST_SYSTEM=system, TEST_ARCH=architecture, TEST_LIBC=libc, KYLER_INSTALLER_BINARY=str(installer))
            return subprocess.run(["/bin/sh", str(INSTALLER), "--non-interactive"], env=env, capture_output=True, text=True, timeout=5)

    def test_supported_platforms_and_architecture_aliases(self):
        for system, architecture in (("Linux", "x86_64"), ("Linux", "aarch64"), ("Darwin", "arm64"), ("Darwin", "x86_64")):
            with self.subTest(system=system, architecture=architecture):
                result = self.check(system, architecture)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_unsupported_host_or_architecture_stops(self):
        for kwargs in ({"system": "FreeBSD"}, {"architecture": "armv7l"}):
            result = self.check(**kwargs)
            self.assertNotEqual(result.returncode, 0)



if __name__ == "__main__":
    unittest.main()
