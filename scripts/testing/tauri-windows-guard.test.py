"""Pure fixture gate tests: synthetic env/local temporary files, never wsl.exe."""
import importlib.util
import contextlib
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('windows_fixture',HERE/'tauri-windows-wsl-smoke.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
EXPECTED='splitty-native-fixture-000000000000-a'

class GateTests(unittest.TestCase):
    def run_gate(self,actual,strip_quotes=False):
        with tempfile.TemporaryDirectory(prefix='splitty-gate-pure-') as root:
            marker=Path(root)/'must-only-write-for-exact-target'
            env={'PATH':'/usr/bin:/bin'}
            if actual is not None:env['WSL_DISTRO_NAME']=actual
            # Synthetic environment: does not query a real distro or run a
            # login shell/profile. The only child action writes this temp file.
            guard=fixture.GUEST_GUARD.replace(chr(34),'') if strip_quotes else fixture.GUEST_GUARD
            result=subprocess.run(['/bin/sh','-c',guard,'fixture-guard',EXPECTED,'/bin/sh','-c','printf touched > "$1"','fixture-mutation',str(marker)],env=env,capture_output=True)
            return result.returncode,marker.exists()
    def test_exact_target_allows_child(self):self.assertEqual(self.run_gate(EXPECTED),(0,True))
    def test_wrong_target_never_mutates(self):self.assertEqual(self.run_gate('different-fixture'),(90,False))
    def test_absent_target_never_mutates(self):self.assertEqual(self.run_gate(None),(90,False))
    def test_stripped_quotes_still_refuse_wrong_or_absent_target(self):
        self.assertEqual(self.run_gate('different-fixture',True),(90,False))
        self.assertEqual(self.run_gate(None,True),(90,False))
    def test_arguments_gate_before_command(self):
        args=fixture.guarded_guest_arguments(EXPECTED,['touch','marker'])
        self.assertEqual(args[:6],['--distribution',EXPECTED,'--exec','/bin/sh','-c',fixture.GUEST_GUARD])
        self.assertEqual(args[6:],['splitty-fixture-guard',EXPECTED,'touch','marker'])
    def test_no_user_distro_argument_accepted(self):
        with self.assertRaises(RuntimeError):fixture.guarded_guest_arguments('Ubuntu',['touch','marker'])
    def test_full_ui_is_opt_in(self):
        seen=[]
        with patch.object(fixture,'_run_full',side_effect=lambda args:seen.append(args.full_ui)):
            fixture.main(['--disposable-distros'])
            fixture.main(['--disposable-distros','--full-ui'])
        self.assertEqual(seen,[False,True]);self.assertFalse(fixture.WSL_EXECUTION_APPROVED)
    def test_full_ui_copies_real_assets_and_keeps_setup_controller(self):
        import shutil
        repo=HERE.parents[1]
        with tempfile.TemporaryDirectory(prefix='splitty-full-ui-pure-') as directory:
            root=Path(directory)/'snapshot';crate=root/'crates/splitty-tauri';(crate/'src').mkdir(parents=True)
            for name in ['lib.rs','windows_wsl.rs','navigation.rs','remote.rs']:shutil.copyfile(repo/'crates/splitty-tauri/src'/name,crate/'src'/name)
            for name in ['build.rs','tauri.conf.json']:shutil.copyfile(repo/'crates/splitty-tauri'/name,crate/name)
            fake_repo=Path(directory)/'input';dist=fake_repo/'frontend/dist';(dist/'assets').mkdir(parents=True)
            (dist/'index.html').write_text('<body><div id="app"></div><script type="module" src="/assets/real.js"></script></body>')
            (dist/'popout.html').write_text('<body>real production child</body>')
            (dist/'assets/real.js').write_text('production-ui-marker')
            (dist/'assets/ghostty.wasm').write_bytes(b'wasm-fixture-preserved')
            with patch.object(fixture.subprocess,'run') as child,patch.object(fixture.common,'windows',side_effect=lambda path:'C:\\Fixture\\'+Path(path).name):
                fixture.instrument(root,[EXPECTED,'splitty-native-fixture-000000000000-b'],fake_repo,full_ui=True)
                child.assert_called_once() # bundler call mocked; no process/WSL
            assets=crate/'runtime-assets'
            self.assertEqual((assets/'assets/ghostty.wasm').read_bytes(),b'wasm-fixture-preserved')
            self.assertIn('real.js',(assets/'index.html').read_text());self.assertIn('/probe.js',(assets/'index.html').read_text())
            self.assertNotIn('id="app"',(assets/'setup.html').read_text())
            self.assertIn('real production child',(assets/'popout.html').read_text())
            self.assertIn('nativeCount',(crate/'src/lib.rs').read_text())
            self.assertIn('.window_features(features).visible(false)',(crate/'src/navigation.rs').read_text())
            self.assertIn('allow-smoke-windows',(crate/'src/lib.rs').read_text())
            self.assertNotIn(r'\nfunction',(crate/'src/windows-full-ui-init.js').read_text())
            self.assertTrue((root/'production-ui-assets.json').is_file())
    def test_launch_argv_keeps_fresh_root_lexical_target_path(self):
        root=Path('/mnt/c/Temp/splitty-native-windows-123456789abc')
        lexical=r'C:\Temp\splitty-native-windows-123456789abc'
        def convert(path):
            if path==root:return lexical
            self.fail('converter followed a target junction instead of using the lexical root')
        with patch.object(fixture.common,'windows',side_effect=convert) as convert_call:
            self.assertEqual(fixture.fixture_executable_path(root),lexical+r'\target\debug\splitty-gui.exe')
            convert_call.assert_called_once_with(root)
    def test_isolation_opt_in_implies_production_assets(self):
        seen=[]
        with patch.object(fixture,'_run_full',side_effect=lambda args:seen.append((args.full_ui,args.isolation_consumer))):
            fixture.main(['--disposable-distros','--isolation-consumer'])
        self.assertEqual(seen,[(True,True)]);self.assertFalse(fixture.WSL_EXECUTION_APPROVED)
    def test_private_host_config_never_uses_user_ssh_files(self):
        with tempfile.TemporaryDirectory(prefix='splitty-ssh-config-pure-') as directory:
            root=Path(directory)
            def keygen(command,**kwargs):
                (Path(command[-1]).with_suffix('.pub')).write_text('ssh-ed25519 PUBLIC-FIXTURE')
            with patch.object(fixture.subprocess,'run',side_effect=keygen),patch.object(fixture.common,'powershell',side_effect=['34567','']),patch.object(fixture.common,'windows',side_effect=lambda path:'C:\\Fixture\\'+Path(path).name):
                host=fixture.prepare_isolation_host(root)
            config=host['config'].read_text()
            self.assertIn('\n HostName 127.0.0.1\n',config)
            self.assertIn('GlobalKnownHostsFile none',config);self.assertIn('IdentityAgent none',config)
            self.assertIn('C:\\Fixture\\known_hosts',config)
    def test_isolation_instrumentation_keeps_production_paths_and_owned_local_scope(self):
        import shutil
        repo=HERE.parents[1]
        with tempfile.TemporaryDirectory(prefix='splitty-isolation-source-pure-') as directory:
            root=Path(directory);crate=root/'crates/splitty-tauri';(crate/'src').mkdir(parents=True);remote=root/'crates/splitty-remote/src';remote.mkdir(parents=True)
            for name in ['lib.rs','client_ipc.rs','navigation.rs','remote.rs']:shutil.copyfile(repo/'crates/splitty-tauri/src'/name,crate/'src'/name)
            shutil.copyfile(repo/'crates/splitty-tauri/build.rs',crate/'build.rs');shutil.copyfile(repo/'crates/splitty-remote/src/ssh.rs',remote/'ssh.rs')
            host={'aliases':['one','two'],'config':root/'private-config'}
            with patch.object(fixture.common,'windows',side_effect=lambda path:'C:\\Fixture\\'+Path(path).name):fixture.instrument_isolation(root,crate,host)
            self.assertIn('fixture_external_open',(crate/'src/navigation.rs').read_text())
            self.assertIn('windows_fixture_state',(crate/'src/lib.rs').read_text())
            self.assertIn('setup_proof',(crate/'src/lib.rs').read_text())
            self.assertIn('.arg("-F")',(remote/'ssh.rs').read_text())
            self.assertIn('smoke_counts',(crate/'src/client_ipc.rs').read_text())
            hidden=(crate/'src/remote.rs').read_text()
            self.assertNotIn('window.show()',hidden)
            self.assertNotIn('window.set_focus()',hidden)
            self.assertNotIn('window.unminimize()',hidden)
            self.assertIn('.visible(false)',hidden)
    def test_listener_inventory_brackets_retained_handle_generations_without_waiver(self):
        source=(HERE/'tauri-windows-native-smoke/wsl-runtime.ps1').read_text()
        before=source.index('$before=@($app.History()')
        sample=source.index('$socketRows=@(Get-FixtureTcpRows')
        after=source.index('$after=@($app.History()')
        self.assertLess(before,sample);self.assertLess(sample,after)
        for expected in ['SocketCreationUTC','BeforeUTC','AfterUTC','RemoteAddress','RemotePort',"'UNCLASSIFIED'",'Test-FixtureSocketGeneration $pre $post $socketCreated $afterAt','-or $listeners.Count']:
            self.assertIn(expected,source)
        self.assertNotIn('Get-NetTCPConnection',source)
        self.assertIn("if($stateKind-eq 'non-listening')",source)
        self.assertIn('InventoryError=$inventoryError',source)
    def test_replay_critical_sequence_has_only_write_then_open_awaits(self):
        import re
        source=(HERE/'tauri-windows-native-smoke/consumer.ts').read_text()
        marker=source.index("data:\"printf 'AFTER_")
        start=source.rfind("await healthyRequest('session.send'",0,marker)
        end=source.index("replay.send({type:'attach'",marker)
        critical=source[start:end]
        self.assertEqual(len(re.findall(r'\bawait\b',critical)),2)
        self.assertIn('await openTauriChannel(bridge)',critical)
        self.assertNotIn("healthyRequest('job.list')",critical)
        self.assertNotIn("await checkpoint(",critical)
    def test_listener_failure_continuation_never_changes_safety_requirements(self):
        import copy
        value={'phase':'final','terminalBinary':True,'savedPath':'/home/splitty-fixture/file'}
        process={'TimedOut':False,'AppInstances':[{'ExitCode':0,'Alive':False}],'RemainingOwned':[],'IndependentFixturePreserved':True,'JobEmptyAfterCleanup':True,'OwnedHandlesAfterCleanup':0,'CleanupErrors':[],'Listeners':[{'Classification':'UNCLASSIFIED'}]}
        self.assertTrue(fixture.listener_only_evidence_continuation(process,value,1))
        for report in [None,{},dict(value,error='failed'),dict(value,phase='isolation-complete')]:
            self.assertFalse(fixture.listener_only_evidence_continuation(process,report,1))
        for key,bad in [('TimedOut',True),('AppInstances',[{'ExitCode':1,'Alive':False}]),('RemainingOwned',[{}]),('IndependentFixturePreserved',False),('JobEmptyAfterCleanup',False),('OwnedHandlesAfterCleanup',1),('CleanupErrors',['bad']),('Listeners',[])]:
            changed=copy.deepcopy(process);changed[key]=bad
            self.assertFalse(fixture.listener_only_evidence_continuation(changed,value,1),key)
        for key in process:
            changed=copy.deepcopy(process);del changed[key]
            self.assertFalse(fixture.listener_only_evidence_continuation(changed,value,1),key)
    def test_structured_error_diagnostics_are_bounded_and_redacted(self):
        import subprocess,json
        module=(HERE/'tauri-windows-native-smoke/error.mjs').as_uri()
        program="""import {fixtureError} from MODULE;
const input={message:'install failed',hint:'missing helper',auth:'private-proof',token:'secret-token',other:'sensitive'};input.cause=input;
const text=fixtureError(input),value=JSON.parse(text);
if(value.message!=='install failed'||value.hint!=='missing helper'||text.includes('private-proof')||text.includes('secret-token')||text.includes('sensitive')||value.cause!=='[bounded]')throw Error(text);
if(fixtureError({message:'x'.repeat(10000)}).length>4096)throw Error('unbounded');
if(!fixtureError(new Error('native failure')).includes('native failure'))throw Error('Error lost');
""".replace('MODULE',json.dumps(module))
        subprocess.run(['node','--input-type=module','-e',program],check=True,timeout=5)
    def test_persistent_ssh_host_uses_retained_job_and_authenticated_readiness(self):
        driver=(HERE/'tauri-windows-native-smoke/wsl-runtime.ps1').read_text()
        source=(HERE/'tauri-windows-wsl-smoke.py').read_text()
        self.assertNotIn("exec /usr/sbin/sshd",source)
        self.assertIn("(' -PermitIsolatedSshHost' if isolation else '')",source)
        for expected in ["if($PermitIsolatedSshHost)","'/usr/sbin/sshd','-D'",'Get-FixtureGuestArguments $hostName',"'-F',$sshConfig",'splitty-isolation-one','$attempt-lt 4','/home/splitty-fixture','Authenticated=$false','$stdout-ceq $nonce','$sshServer.Stop(5000)','$sshServer.Dispose()',"'ssh-host.json'"]:
            self.assertIn(expected,driver)
        self.assertLess(driver.index('$sshServer=[SplittyFixtureJob]::new'),driver.index('$app=[SplittyFixtureJob]::new'))
        self.assertLess(driver.index('if(-not $sshEvidence.Authenticated)'),driver.index('$app=[SplittyFixtureJob]::new'))
        self.assertIn("if($guestJob.ActiveProcesses-ne 0){throw 'gated guest command deadline'}",(HERE/'tauri-windows-native-smoke/fixture-sentinel.ps1').read_text())
    def test_imported_full_entry_refuses_before_windows_prep(self):
        fixture.WSL_EXECUTION_APPROVED=False
        with patch.object(fixture.common,'powershell',side_effect=AssertionError('Windows prep called')):
            with self.assertRaisesRegex(RuntimeError,'scoped approval'):fixture._run_full(None)
    def test_cli_no_flag_never_enables_or_runs(self):
        fixture.WSL_EXECUTION_APPROVED=False
        with patch.object(fixture,'_run_full') as run,patch.object(fixture.common,'powershell',side_effect=AssertionError('Windows call')),contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:fixture.main([])
            self.assertEqual(error.exception.code,2)
            run.assert_not_called()
        self.assertFalse(fixture.WSL_EXECUTION_APPROVED)
    def test_cli_opt_in_scopes_then_resets_approval(self):
        fixture.WSL_EXECUTION_APPROVED=False
        seen=[]
        with patch.object(fixture,'_run_full',side_effect=lambda _args:seen.append(fixture.WSL_EXECUTION_APPROVED)):
            fixture.main(['--disposable-distros'])
        self.assertEqual(seen,[True]);self.assertFalse(fixture.WSL_EXECUTION_APPROVED)
    def test_cli_failure_resets_approval(self):
        fixture.WSL_EXECUTION_APPROVED=False
        def fail(_args):
            self.assertTrue(fixture.WSL_EXECUTION_APPROVED)
            raise RuntimeError('synthetic fixture failure')
        with patch.object(fixture,'_run_full',side_effect=fail):
            with self.assertRaisesRegex(RuntimeError,'synthetic'):fixture.main(['--disposable-distros'])
        self.assertFalse(fixture.WSL_EXECUTION_APPROVED)
    def test_approval_requires_literal_opt_in_and_refuses_nested_runs(self):
        fixture.WSL_EXECUTION_APPROVED=False
        with self.assertRaises(RuntimeError):
            with fixture.approved_private_wsl(False):self.fail('entered without opt-in')
        with fixture.approved_private_wsl(True):
            with self.assertRaises(RuntimeError):
                with fixture.approved_private_wsl(True):self.fail('nested fixture run')
            self.assertTrue(fixture.WSL_EXECUTION_APPROVED)
        self.assertFalse(fixture.WSL_EXECUTION_APPROVED)
    def test_copied_application_executor_and_owner_both_gate_first(self):
        repo=HERE.parents[1]
        original=(repo/'crates/splitty-tauri/src/windows_wsl.rs').read_text()
        with tempfile.TemporaryDirectory(prefix='splitty-rust-guard-pure-') as directory:
            root=Path(directory);crate=root/'crates/splitty-tauri';(crate/'src').mkdir(parents=True);(crate/'runtime-assets').mkdir()
            (crate/'src/windows_wsl.rs').write_text(original)
            (crate/'src/lib.rs').write_text('#[cfg(target_os = "windows")]\n#[tauri::command]\nfn windows_smoke_report() {}\n')
            with patch.object(fixture.common,'instrument'),patch.object(fixture.subprocess,'run') as child:
                fixture.instrument(root,[EXPECTED,'splitty-native-fixture-000000000000-b'],repo)
                child.assert_called_once() # mock only: never launches Vite
            guarded=(crate/'src/windows_wsl.rs').read_text()
            self.assertEqual(guarded.count(__import__('json').dumps(fixture.GUEST_GUARD)),2)
            self.assertIn('let arguments = &fixture_arguments;',guarded)
            self.assertIn('fn native_backend_arguments(distro: &str, binary: &str) -> Vec<String>',guarded)
            self.assertIn('assert!(names.iter().any(|name| name == distro)',guarded)
            self.assertIn('"splitty-fixture-guard".into(), distro.into(), binary.into()',guarded)
        self.assertEqual((repo/'crates/splitty-tauri/src/windows_wsl.rs').read_text(),original)
    def test_runtime_disabled_before_any_windows_call(self):
        fixture.common.powershell=lambda *_:self.fail('Windows call during disabled test')
        with self.assertRaises(RuntimeError):fixture.wsl(EXPECTED,['touch','marker'])
        with self.assertRaises(RuntimeError):
            with fixture.fixtures(Path('/unused')):self.fail('fixture ran')

if __name__=='__main__':unittest.main()
