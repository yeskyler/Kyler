"""Isolated local-installer tests: python3 scripts/tests/test_install.py."""
import os
import json
import hashlib
import shutil
import socket
import queue
import threading
import time
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
import urllib.request

WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
INSTALLER = WORKSPACE_ROOT / "scripts/install.sh"
KYLER_BINARY = WORKSPACE_ROOT / "kyler-rs/target/debug/kyler"


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

    def install(self, *args, success=True):
        result = subprocess.run(
            ["sh", str(INSTALLER), "--local", str(self.source), "--no-start", "--no-configure", *args],
            env=self.env, capture_output=True, text=True, timeout=10,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def test_first_install_creates_private_config_without_editing_shell(self):
        result = self.install()
        binary = self.home / ".local/bin/kyler"
        config = self.home / ".kyler/config.toml"
        self.assertEqual(binary.read_bytes(), self.source.read_bytes())
        self.assertTrue(os.access(binary, os.X_OK))
        self.assertEqual(stat.S_IMODE(config.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(config.parent.stat().st_mode), 0o700)
        self.assertIn("enabled = true", config.read_text())
        self.assertNotIn("api_key =", "\n".join(line for line in config.read_text().splitlines() if not line.startswith("#")))
        self.assertFalse((self.home / ".zshrc").exists())
        self.assertFalse((self.home / ".kyler/sessions").exists())
        self.assertIn('export PATH="$HOME/.local/bin:$PATH"', result.stdout)
        self.assertFalse(list(binary.parent.glob(".kyler-install.*")))
        self.assertFalse(list(config.parent.glob(".config-install.*")))

    def test_upgrade_preserves_config_credentials_sessions_and_adds_path_once(self):
        self.install("--modify-path")
        config = self.home / ".kyler/config.toml"
        config.write_text("# custom config\n[llm]\napi_key='PRIVATE_TEST_KEY'\n")
        secret = self.home / ".kyler/.env"
        secret.write_text("PRIVATE_TEST_ENV=value\n")
        session = self.home / ".kyler/sessions/session/rollout.jsonl"
        session.parent.mkdir(parents=True)
        session.write_text("history\n")
        before = [path.read_bytes() for path in (config, secret, session)]
        self.source.write_text('#!/bin/sh\nprintf "kyler 0.2.0\\n"\n')
        result = self.install("--modify-path")
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
        result = subprocess.run(["sh", str(INSTALLER), "--local"], env=self.env, capture_output=True)
        self.assertNotEqual(result.returncode, 0)

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
        for key in ("KYLER_CONFIG", "KYLER_SESSION_DIR", "KYLER_SESSION_ENABLED", "KYLER_SERVER_TOKEN", "KYLER_INTERNAL_TOOL_WORKER"):
            environment.pop(key, None)
        environment["KYLER_SERVER_BIND"] = "127.0.0.1:0"
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
    def test_default_install_configures_starts_and_restarts_background_service(self):
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
        command = ["sh", str(INSTALLER), "--local", str(binary), "--non-interactive"]
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
                with opener.open(record["url"] + "/v1/info", timeout=5) as response:
                    info = json.load(response)
                self.assertEqual(info["instance_id"], first_instance)
                self.assertEqual(info["model"], "installer-model")
                self.assertIsNone(info["workspace_root"])
                self.assertIsNone(info["working_directory"])
                self.assertEqual(info["default_workspace"], str((self.home / ".kyler/workspace").resolve()))
                self.assertTrue((self.home / ".kyler/workspace").is_dir())
            self.assertIn("INSTALLER_TEST_KEY", (self.home / ".kyler/config.toml").read_text())
            self.assertTrue((self.home / ".kyler/logs/server.log").is_file())
        finally:
            if installed.exists():
                subprocess.run([str(installed), "service", "stop"], env=environment, capture_output=True, timeout=15)

    def interactive_install(self, replies, expected=0, piped=False, redirected=False, bundle=None):
        replies = list(replies)
        if expected == 0:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                address = "127.0.0.1:" + str(listener.getsockname()[1])
            replies += [(b"[1] Backend address", (address + "\n").encode())]
            if bundle is not None:
                with socket.socket() as listener:
                    listener.bind(("127.0.0.1", 0))
                    web_address = "127.0.0.1:" + str(listener.getsockname()[1])
                replies += [(b"[2] Web address", (web_address + "\n").encode())]
        import pty
        import select
        import termios
        import fcntl
        binary = KYLER_BINARY
        environment = self.env.copy()
        for name in list(environment):
            if name.startswith("KYLER_") or name == "OPENAI_API_KEY":
                environment.pop(name)
        master, slave = pty.openpty()
        before = termios.tcgetattr(slave)

        def assert_terminal_restored():
            actual = termios.tcgetattr(slave)
            expected_attributes = list(before)
            # macOS may set PENDIN when returning to canonical input; it is kernel queue state.
            pending = getattr(termios, "PENDIN", 0)
            actual[3] &= ~pending
            expected_attributes[3] &= ~pending
            self.assertEqual(actual, expected_attributes)

        command = ["sh", str(INSTALLER), "--local", str(binary), "--no-start"]
        if bundle is not None:
            command = ["sh", str(INSTALLER), "--bundle", str(bundle)]
        if piped:
            command = ["sh", "-c", 'cat "$1" | sh -s -- --local "$2" --no-start', "installer-pipe", str(INSTALLER), str(binary)]
        elif redirected:
            command = ["sh", "-c", 'sh "$1" --local "$2" --no-start </dev/tty', "installer-tty", str(INSTALLER), str(binary)]

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        process = subprocess.Popen(
            command,
            cwd=self.root, env=environment, stdin=slave, stdout=slave, stderr=slave,
            preexec_fn=controlling_terminal if piped or redirected else None,
        )
        output = b""
        cursor = 0
        try:
            for marker, answer in replies:
                deadline = time.monotonic() + 15
                while marker not in output[cursor:]:
                    if process.poll() is not None or time.monotonic() > deadline:
                        self.fail("Installer did not show prompt " + repr(marker))
                    if select.select([master], [], [], 0.1)[0]:
                        output += os.read(master, 65536)
                cursor = output.index(marker, cursor) + len(marker)
                if (piped or redirected) and marker == b"[1] Backend address":
                    assert_terminal_restored()
                if marker.startswith(b"["):
                    step = int(marker.split(b"]", 1)[0][1:])
                    self.assertNotIn(("[%d] " % (step + 1)).encode(), output[cursor:], "A later prompt appeared before the current answer passed")
                os.write(master, answer)
            deadline = time.monotonic() + 15
            while process.poll() is None and time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    output += os.read(master, 65536)
            self.assertEqual(process.poll(), expected)
            while select.select([master], [], [], 0.05)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError as error:
                    if error.errno == 5:
                        break
                    raise
                if not chunk:
                    break
                output += chunk
            # A session leader's exit hangs up its controlling PTY on macOS.
            # Those cases check restoration above while the next prompt is live.
            if not piped and not redirected:
                assert_terminal_restored()
            return output
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_interactive_install_hides_key_and_restores_terminal(self):
        output = self.interactive_install([
            (b"[1] OPENAI_BASE_URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[2] OPENAI_API_KEY", b"PTY_SECRET_TEST_KEY\r"),
            (b"[3] Model [", b"pty-model\n"),
        ])
        self.assertNotIn(b"PTY_SECRET_TEST_KEY", output)
        self.assertNotIn(b"\x1b[?1049h", output)
        self.assertNotIn(b"\x1b[2J", output)
        self.assertNotIn(b"[1/3]", output)
        config = (self.home / ".kyler/config.toml").read_text()
        self.assertIn("PTY_SECRET_TEST_KEY", config)
        self.assertNotIn("tools.tavily", config)
        for marker in [b"[1] OPENAI_BASE_URL", b"[2] OPENAI_API_KEY", b"[3] Model", b"kyler tui", b"kyler service stop"]:
            self.assertIn(marker, output)

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_piped_installer_reads_hidden_key_from_controlling_terminal(self):
        output = self.interactive_install([
            (b"[1] OPENAI_BASE_URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[2] OPENAI_API_KEY", b"PIPE_SECRET_TEST_KEY\r"),
            (b"[3] Model [", b"pipe-model\n"),
        ], piped=True)
        self.assertNotIn(b"PIPE_SECRET_TEST_KEY", output)
        self.assertIn("PIPE_SECRET_TEST_KEY", (self.home / ".kyler/config.toml").read_text())

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_dev_tty_input_does_not_require_event_reader_initialization(self):
        output = self.interactive_install([
            (b"[1] OPENAI_BASE_URL [", b"http://127.0.0.1:1/v1\n"),
            (b"[2] OPENAI_API_KEY", b"TTY_SECRET_TEST_KEY\r"),
            (b"[3] Model [", b"tty-model\n"),
        ], redirected=True)
        self.assertNotIn(b"TTY_SECRET_TEST_KEY", output)
        self.assertIn("TTY_SECRET_TEST_KEY", (self.home / ".kyler/config.toml").read_text())

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_setup_retries_invalid_input_without_extra_configuration_steps(self):
        output = self.interactive_install([
            (b"[1] OPENAI_BASE_URL [", b"not-a-url\n"),
            (b"[1] OPENAI_BASE_URL [", b"https://example.com/v1/chat/completions\n"),
            (b"[1] OPENAI_BASE_URL [", b"  http://127.0.0.1:1/v1/  \n"),
            (b"[2] OPENAI_API_KEY", b"\r"),
            (b"[2] OPENAI_API_KEY", b"\x1b[200~first\nsecond\x1b[201~\r"),
            (b"[2] OPENAI_API_KEY", b"\x1b[200~  RETRY_SECRET_KEY\n\x1b[201~\r"),
            (b"[3] Model [", b"  pty-model  \n"),
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
        config.write_text('[llm]\nmodel="original-model"\nbase_url="https://example.com/v1"\napi_key="ORIGINAL_TEST_KEY"\n')
        before = config.read_bytes()
        installed = self.home / ".local/bin/kyler"
        binary_before = installed.read_bytes()
        output = self.interactive_install([
            (b"[1] OPENAI_BASE_URL [", b"\n"),
            (b"[2] OPENAI_API_KEY", b"\x1b"),
        ], expected=130)
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(installed.read_bytes(), binary_before)
        self.assertNotIn(b"ORIGINAL_TEST_KEY", output)
        self.assertFalse(list(installed.parent.glob(".kyler-install.*")))

    @unittest.skipUnless(os.name == "posix" and (KYLER_BINARY).is_file(), "requires PTY and kyler binary")
    def test_enter_keeps_existing_values_and_hidden_keys(self):
        self.install()
        config = self.home / ".kyler/config.toml"
        config.write_text('[llm]\nmodel="kept-model"\nbase_url="https://example.com/v1"\napi_key="KEPT_MODEL_KEY"\n[tools.tavily]\nenabled=true\napi_key="KEPT_SEARCH_KEY"\n[context]\ncontext_tokens=64000\n')
        output = self.interactive_install([
            (b"[1] OPENAI_BASE_URL [", b"\n"),
            (b"[2] OPENAI_API_KEY", b"\r"),
            (b"[3] Model [", b"\n"),
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
        config.write_text('[llm]\nmodel="kept-model"\nbase_url="https://example.com/v1"\napi_key="invalid saved key"\n')
        output = self.interactive_install([
            (b"[1] OPENAI_BASE_URL [", b"\n"),
            (b"[2] OPENAI_API_KEY", b"\r"),
            (b"[2] OPENAI_API_KEY", b"REPLACEMENT_TEST_KEY\r"),
            (b"[3] Model [", b"\n"),
        ])
        self.assertIn(b"Saved key is invalid", output)
        self.assertEqual(output.count(b"[2] OPENAI_API_KEY"), 2)
        self.assertNotIn(b"Backend URL", output)
        self.assertNotIn(b"invalid saved key", output)
        self.assertNotIn(b"REPLACEMENT_TEST_KEY", output)
        self.assertIn('api_key = "REPLACEMENT_TEST_KEY"', config.read_text())

    def test_server_start_failure_shows_reason_and_recovery_commands(self):
        self.source.write_text("#!/bin/sh\ncase \"$*\" in\n*--version*) echo 'kyler 0.1.0' ;;\n*) echo 'Server address is in use.' >&2; exit 2 ;;\nesac\n")
        result = subprocess.run(
            ["sh", str(INSTALLER), "--local", str(self.source), "--no-configure"],
            env=self.env, capture_output=True, text=True, timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Server address is in use.", result.stderr)
        self.assertIn("kyler service status", result.stderr)
        self.assertIn("~/.kyler/logs/server.log", result.stderr)

    def test_bash_and_unknown_shell_startup_files(self):
        self.env["SHELL"] = "/bin/bash"
        self.install("--modify-path")
        self.assertTrue((self.home / ".bashrc").is_file())
        self.env["SHELL"] = "/bin/sh"
        self.install("--modify-path")
        self.assertTrue((self.home / ".profile").is_file())


@unittest.skipUnless((KYLER_BINARY).is_file() and shutil.which("node"), "requires kyler binary and Node")
class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="kyler bundle ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("KYLER_") and key != "OPENAI_API_KEY"}
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
        script = '''const http = require('node:http');
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
        (bundle / "web/server.js").write_text("process.exit(42);" if broken else script)
        self.manifest(bundle)
        return bundle

    @staticmethod
    def manifest(bundle):
        entries = []
        for path in sorted(bundle.rglob("*")):
            if path.is_file() and path.name != "SHA256SUMS":
                entries.append(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + path.relative_to(bundle).as_posix())
        (bundle / "SHA256SUMS").write_text("\n".join(entries) + "\n")

    def install(self, bundle, success=True):
        result = subprocess.run(["sh", str(INSTALLER), "--bundle", str(bundle), "--non-interactive"], env=self.env, capture_output=True, text=True, timeout=90)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        self.assertNotIn("BUNDLE_TEST_KEY", result.stdout + result.stderr)
        return result

    def status(self, *args):
        result = subprocess.run([str(self.binary), "--json", "service", *args], env=self.env, capture_output=True, text=True, timeout=50)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

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
        self.assertIn("Previous installation restored", result.stderr)
        self.assertEqual(config.read_bytes(), before)
        self.assertEqual(current.resolve(), version)
        self.env.pop("KYLER_WEB_BIND")
        state = self.status("status")
        self.assertTrue(state["running"])
        self.assertEqual(stat.S_IMODE((self.home / ".kyler/web.json").stat().st_mode), 0o600)
        self.assertFalse(self.status("stop")["running"])
        self.assertFalse(self.status("status")["web"]["running"])

    @unittest.skipUnless(os.name == "posix", "requires PTY")
    def test_interactive_bundle_uses_three_stages_and_starts_services(self):
        self.env.pop("KYLER_SERVER_BIND")
        self.env.pop("KYLER_WEB_BIND")
        output = InstallTests.interactive_install(self, [
            (b"[1] OPENAI_BASE_URL [https://api.siliconflow.cn/v1]", b"\n"),
            (b"[2] OPENAI_API_KEY", b"WIZARD_TEST_KEY\r"),
            (b"[3] Model [deepseek-ai/DeepSeek-V4-Flash]", b"\n"),
        ], bundle=self.bundle("interactive"))
        titles = [b"Configure model", b"Configure service addresses", b"Service status and management"]
        self.assertEqual([output.index(title) for title in titles], sorted(output.index(title) for title in titles))
        self.assertIn(b"OpenAI-compatible Chat Completions API", output)
        self.assertNotIn(b"WIZARD_TEST_KEY", output)
        self.assertNotIn(b"Backend URL", output)
        self.assertNotIn(b"next page", output.lower())
        self.assertIn(b"kyler service status", output)
        self.assertIn(b"kyler service start --restart", output)
        config = (self.home / ".kyler/config.toml").read_text()
        self.assertIn('model = "deepseek-ai/DeepSeek-V4-Flash"', config)
        self.assertIn('base_url = "https://api.siliconflow.cn/v1"', config)
        state = self.status("status")
        self.assertTrue(state["backend"]["running"])
        self.assertTrue(state["web"]["running"])
        self.assertIn(state["backend"]["url"].encode(), output)
        self.assertIn(state["web"]["url"].encode(), output)
        self.assertIn('server_url = "' + state["backend"]["url"] + '"', config)

    def test_corrupt_bundle_is_rejected_before_installation(self):
        bundle = self.bundle("corrupt")
        (bundle / "web/server.js").write_text("modified")
        self.install(bundle, success=False)
        self.assertFalse(self.binary.exists())


if __name__ == "__main__":
    unittest.main()
