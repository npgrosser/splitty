#!/usr/bin/env python3
"""Portable preparation checks only. Never starts WSL or changes configuration."""
import hashlib
import base64
import importlib.util
import json
import io
import os
from unittest.mock import patch
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

class NativePreparedInputs(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('wsl_native_fixture', HERE.parent/'tauri-windows-wsl-smoke.py')
        self.fixture = importlib.util.module_from_spec(spec); spec.loader.exec_module(self.fixture)
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)/'checkout'; (self.repo/'frontend').mkdir(parents=True)
        (self.repo/'Cargo.lock').write_bytes(b'pinned\n')
        self.artifact = Path(self.temp.name)/'artifact'; self.artifact.mkdir()
        (self.artifact/'splitty').write_bytes(b'prepared-linux-executable')
        (self.artifact/'app-rootfs.tar').write_bytes(b'prepared-rootfs')
        with tarfile.open(self.artifact/'frontend-dist.tar','w') as archive:
            data=b'<body>real-packaged-assets</body>'; info=tarfile.TarInfo('index.html'); info.size=len(data)
            archive.addfile(info,io.BytesIO(data))
        self.value={'sourceSha':'pinned-source','cargoLockSha256':hashlib.sha256(b'pinned\n').hexdigest(),
                    'files':{name:hashlib.sha256((self.artifact/name).read_bytes()).hexdigest()
                             for name in ['splitty','app-rootfs.tar','frontend-dist.tar']}}
        self.write_manifest()
        self.environment={'GITHUB_ACTIONS':'true','RUNNER_ENVIRONMENT':'github-hosted','RUNNER_OS':'Windows',
                          'GITHUB_SHA':'pinned-source','GITHUB_WORKSPACE':str(self.repo)}
    def write_manifest(self): (self.artifact/'app-manifest.json').write_text(json.dumps(self.value))
    def load(self):
        with patch.object(self.fixture.common,'NATIVE_WINDOWS',True),patch.dict(os.environ,self.environment,clear=True):
            return self.fixture.load_hosted_artifacts(self.artifact,self.repo)
    def test_verified_assets_load_without_linux_commands(self):
        with patch.object(self.fixture.subprocess,'run',side_effect=AssertionError('no child expected')):
            self.load()
        self.assertIn('real-packaged-assets',(self.repo/'frontend/dist/index.html').read_text())
    def test_revision_and_digest_fail_before_modifying_existing_assets(self):
        dist=self.repo/'frontend/dist';dist.mkdir();(dist/'sentinel').write_text('unchanged')
        self.value['sourceSha']='other';self.write_manifest()
        with self.assertRaisesRegex(RuntimeError,'source revision'): self.load()
        self.value['sourceSha']='pinned-source';self.write_manifest();(self.artifact/'splitty').write_bytes(b'changed')
        with self.assertRaisesRegex(RuntimeError,'digest mismatch'): self.load()
        self.assertEqual((dist/'sentinel').read_text(),'unchanged')
    def test_native_host_and_checkout_guards_refuse_local_sources(self):
        with self.assertRaisesRegex(RuntimeError,'hosted native Windows'):
            self.fixture.load_hosted_artifacts(self.artifact,self.repo)
        self.environment['GITHUB_WORKSPACE']=str(self.repo/'other')
        with self.assertRaisesRegex(RuntimeError,'fresh hosted checkout'): self.load()
    def test_windows_checkout_lock_newlines_preserve_canonical_provenance(self):
        (self.repo/'Cargo.lock').write_bytes(b'pinned\r\n');self.load()
    def test_host_paths_do_not_call_wslpath_on_native_windows(self):
        with patch.object(self.fixture.common,'NATIVE_WINDOWS',True),patch.object(self.fixture.common.subprocess,'check_output',side_effect=AssertionError('no wslpath')):
            self.assertEqual(self.fixture.common.host_path(self.repo),self.repo)
            self.assertEqual(self.fixture.common.windows(self.repo),str(self.repo.absolute()))
    def test_cleanup_rejects_unverified_records_before_registry_or_commands(self):
        name='splitty-native-fixture-000000000000-a'
        with self.fixture.approved_private_wsl(True),patch.object(self.fixture.common,'windows',side_effect=str),patch.object(self.fixture,'registrations',side_effect=AssertionError('no registry access')),patch.object(self.fixture.common,'powershell',side_effect=AssertionError('no commands')):
            with self.assertRaisesRegex(RuntimeError,'invalid recorded cleanup ownership'):
                self.fixture.cleanup_owned_records(self.repo,{}, {name:'outside-owned-root'})
            with self.assertRaisesRegex(RuntimeError,'invalid recorded cleanup ownership'):
                self.fixture.cleanup_owned_records(self.repo,{name:'prior'}, {name:str(self.repo/'distributions'/name)})
    def test_partial_import_cleanup_targets_only_recorded_new_identity(self):
        first='splitty-native-fixture-000000000000-a';second='splitty-native-fixture-000000000000-b'
        owned={name:str(self.repo/'distributions'/name) for name in [first,second]}
        registered={first:owned[first],'preflight-existing':'untouched'};commands=[]
        def command(source,log=None):
            commands.append(source)
            if '--unregister' in source: registered.pop(first)
        with self.fixture.approved_private_wsl(True),patch.object(self.fixture.common,'windows',side_effect=str),patch.object(self.fixture,'registrations',side_effect=lambda:registered.copy()),patch.object(self.fixture.common,'powershell',side_effect=command):
            self.fixture.cleanup_owned_records(self.repo,{'preflight-existing':'untouched'},owned)
        self.assertEqual(registered,{'preflight-existing':'untouched'})
        self.assertEqual(len(commands),2)
        self.assertTrue(all(first in command and second not in command and 'preflight-existing' not in command for command in commands))

    @unittest.skipUnless(Path('/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe').is_file(), 'delegated PowerShell source check requires installed parser/runtime; never WSL')
    def test_delegated_powershell_crlf_source_and_synthetic_guest_gate(self):
        source=(HERE.parent/'tauri-windows-native-smoke/fixture-sentinel.ps1').read_text().replace('\n','\r\n')
        guard=(HERE.parent/'tauri-windows-native-smoke/fixture-guest-guard.sh').read_text().replace('\n','\r\n')
        unit=(HERE/'delegated-guard.test.ps1').read_text()
        code="$Source=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('"+base64.b64encode(source.encode()).decode()+"'));$GuardBase64='"+base64.b64encode(guard.encode()).decode()+"';"+unit[unit.index('$ErrorActionPreference='):]
        result=subprocess.run(['/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe','-NoProfile','-NonInteractive','-EncodedCommand',base64.b64encode(code.encode('utf-16le')).decode()],capture_output=True,timeout=30)
        self.assertEqual(result.returncode,0,result.stderr.decode(errors='replace'))
        arguments=json.loads(base64.b64decode(result.stdout.decode().strip()).decode())
        self.assertNotIn('\r',arguments[5]);self.assertNotIn('\r',arguments[10])
        expected='splitty-native-fixture-000000000000-a'
        for actual,success in [(expected,True),('foreign',False)]:
            with tempfile.TemporaryDirectory() as directory:
                marker=Path(directory)/'marker';guest=arguments[5:];guest[-1]=str(marker)
                command=subprocess.run(['/bin/sh','-c',*guest],env={'PATH':'/usr/bin:/bin','WSL_DISTRO_NAME':actual},capture_output=True)
                self.assertEqual(command.returncode==0,success,command.stderr)
                self.assertEqual(marker.exists(),success)

    def test_native_proof_is_enclosed_by_false_controls(self):
        source=(HERE/'run.ps1').read_text()
        before=source.index("Probe-Forwarding $false 'before-native'")
        invoke=source.index('  Invoke-NativeApplication',before)
        after=source.index("Probe-Forwarding $false 'after-native'",invoke)
        self.assertLess(before,invoke);self.assertLess(invoke,after)
        self.assertIn('NativeApplicationPassed=$nativePassed',source)
        self.assertIn("$report.fullUi-ne $true",source)
        self.assertIn("--cleanup-root",source)

if __name__ == '__main__': unittest.main()
