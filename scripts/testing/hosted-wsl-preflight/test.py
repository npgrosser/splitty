#!/usr/bin/env python3
"""Portable preparation checks only. Never starts WSL or changes configuration."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('fixture_build', HERE / 'build.py')
fixture = importlib.util.module_from_spec(spec); spec.loader.exec_module(fixture)

class Preparation(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.output = Path(cls.temp.name) / 'artifact'; fixture.build(cls.output)
    @classmethod
    def tearDownClass(cls): cls.temp.cleanup()
    def test_archive_has_only_fixture_files_and_nonroot_home(self):
        with tarfile.open(self.output / 'rootfs.tar') as tar:
            for member in tar.getmembers():
                self.assertFalse(member.name.startswith('/'))
                self.assertNotIn('..', Path(member.name).parts)
                self.assertFalse(member.name.startswith('home/') and member.name != 'home/splitty-fixture')
            self.assertEqual(tar.getmember('home/splitty-fixture').uid, 1000)
            self.assertEqual(tar.getmember('home/splitty-fixture').mode, 0o700)
            config = tar.extractfile('etc/wsl.conf').read().decode()
            self.assertIn('enabled=false', config)
            self.assertIn('default=splitty-fixture', config)
            self.assertEqual(tar.extractfile('usr/bin/fixture-net').read(), (self.output/'fixture-net').read_bytes())
    def test_artifact_digests_match(self):
        manifest = json.loads((self.output/'manifest.json').read_text())
        self.assertEqual(manifest['rootfsSha256'], hashlib.sha256((self.output/'rootfs.tar').read_bytes()).hexdigest())
        self.assertEqual(manifest['netSha256'], hashlib.sha256((self.output/'fixture-net').read_bytes()).hexdigest())
    def test_tcp_tool_rejects_invalid_work_before_socket(self):
        for arguments in [[], ['serve','80','a'*32], ['serve','12345','not-a-nonce'], ['other','12345','a'*32]]:
            self.assertEqual(subprocess.run([str(self.output/'fixture-net'), *arguments], timeout=2).returncode, 64)
    def test_driver_keeps_hosted_and_ownership_gates(self):
        source = (HERE/'run.ps1').read_text()
        for expected in ["$env:GITHUB_ACTIONS -cne 'true'", "$env:RUNNER_ENVIRONMENT -cne 'github-hosted'", "$env:RUNNER_OS -cne 'Windows'", 'zero pre-existing distributions', 'Assert-Owned -AllowAbsent', "'--version','2'", "'(?i)microsoft.*wsl2'", 'guest listener disappeared during negative control', 'configuration bytes not restored', 'LiveHandleCount-ne 0']:
            self.assertIn(expected, source)
        self.assertNotIn('Stop-Process', source)
        self.assertNotIn('wsl.exe --shutdown', source)  # commands use owned bounded Job wrapper
        self.assertLess(source.index('Only an explicitly opted-in'), source.index("$config=Join-Path"))

if __name__ == '__main__': unittest.main()
