"""Isolated shell installation tests; no network or model calls."""
import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'install.sh'


class InstallationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / 'home'
        self.home.mkdir()
        self.bundle = self.root / 'bundle'
        for name in ('bin/kyler', 'runtime/node', 'runtime/tools/rg', 'web/server.js'):
            path = self.bundle / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$HOME/calls"\n')
            path.chmod(0o755)
        self.manifest()

    def manifest(self):
        files = sorted(p for p in self.bundle.rglob('*') if p.is_file() and p.name != 'SHA256SUMS')
        (self.bundle / 'SHA256SUMS').write_text(''.join(
            f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(self.bundle)}\n' for p in files))

    def run_install(self, bundle=None, *flags):
        return subprocess.run(['sh', str(SCRIPT), '--bundle', str(bundle or self.bundle), *flags],
                              env=dict(os.environ, HOME=str(self.home), SHELL='/bin/sh'),
                              capture_output=True, text=True)

    def test_configures_then_starts_without_diagnostics(self):
        result = self.run_install(None, '--non-interactive')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.home / 'calls').read_text().splitlines(),
                         ['configure --non-interactive --web --addresses', 'service start --restart'])
        self.assertTrue((self.home / '.local/bin/kyler').is_file())

    def test_archive_and_reinstallation(self):
        archive = self.root / 'bundle.tar.gz'
        with tarfile.open(archive, 'w:gz') as output:
            output.add(self.bundle, arcname='bundle')
        for _ in range(2):
            result = self.run_install(archive, '--no-configure', '--no-start')
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.home / 'calls').exists())

    def test_corruption_prevents_install(self):
        (self.bundle / 'bin/kyler').write_text('corrupt')
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / '.local/bin/kyler').exists())

    def test_unlisted_file_rejected(self):
        (self.bundle / 'extra').write_text('unlisted')
        self.assertNotEqual(self.run_install().returncode, 0)

    def test_links_rejected(self):
        (self.bundle / 'link').symlink_to('/etc/passwd')
        self.assertNotEqual(self.run_install().returncode, 0)

    def test_archive_traversal_rejected(self):
        archive = self.root / 'unsafe.tar.gz'
        with tarfile.open(archive, 'w:gz') as output:
            output.add(self.bundle / 'bin/kyler', arcname='../escaped')
        self.assertNotEqual(self.run_install(archive).returncode, 0)
        self.assertFalse((self.root / 'escaped').exists())

    def test_configuration_failure_does_not_start(self):
        (self.bundle / 'bin/kyler').write_text('#!/bin/sh\nexit 1\n')
        self.manifest()
        self.assertNotEqual(self.run_install(None, '--non-interactive').returncode, 0)
        self.assertTrue((self.home / '.local/bin/kyler').exists())
        self.assertFalse((self.home / 'calls').exists())


if __name__ == '__main__':
    unittest.main()
