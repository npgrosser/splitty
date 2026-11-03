#!/usr/bin/env python3
"""Portable observer/preparation checks; these never claim WKWebView coverage."""
import ctypes
import importlib.util
from pathlib import Path
import unittest
from unittest import mock
import json
import tempfile
import subprocess

spec = importlib.util.spec_from_file_location('wkwebview_smoke', Path(__file__).with_name('tauri-wkwebview-smoke.py'))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class ObserverTests(unittest.TestCase):
    def test_process_tree_preserves_commands_and_excludes_foreign_siblings(self):
        table = smoke.parse_processes(' 10 1 /tmp/smoke app\n11 10 /bin/sh\n12 11 splitty session-keeper\n13 1 foreign\n')
        self.assertEqual(smoke.tree(10, table), {10, 11, 12})
        self.assertEqual(table[10]['command'], '/tmp/smoke app')

    def test_lsof_ipv4_ipv6_and_empty_inventory(self):
        self.assertEqual(smoke.parse_listeners(''), [])
        self.assertEqual(smoke.parse_listeners('p10\ncfixture\nf5\nn127.0.0.1:123\nf6\nn[::1]:456\n'), [
            {'pid':10,'command':'fixture','address':'127.0.0.1:123'},
            {'pid':10,'command':'fixture','address':'[::1]:456'},
        ])

    def test_native_observer_uses_appkit_without_changing_shared_assertions(self):
        source = (Path(__file__).parent/'tauri-webkit-smoke/report.rs').read_text()
        result = smoke.macos_report(source)
        self.assertNotIn('gtk::', result)
        self.assertIn('NSApplication::sharedApplication(', result)
        self.assertIn('application.windows().iter().filter(|window| window.isVisible())', result)
        self.assertEqual(source[:source.index('async fn smoke_windows')], result[:result.index('async fn smoke_windows')])

    def test_fixture_home_resolves_an_actual_symlinked_temporary_root(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root).resolve(strict=True)
            real = root/'actual'
            real.mkdir()
            alias = root/'alias'
            alias.symlink_to(real, target_is_directory=True)
            with tempfile.TemporaryDirectory(dir=alias) as private:
                home = smoke.fixture_home(private)
                self.assertNotEqual(home, Path(private))
                self.assertEqual(home.parent, real)
                self.assertTrue(home.is_dir())
                endpoint = home/'splitty/run/client.sock'
                endpoint.parent.mkdir(parents=True)
                self.assertTrue(all(not ancestor.is_symlink() for ancestor in endpoint.parents))
                self.assertTrue(alias.is_symlink(), 'fixture canonicalization must not alter the alias')

    def test_shared_probe_document_guard_and_main_capsule_assertion(self):
        regression = Path(__file__).parent/'tauri-webkit-smoke/document.test.cjs'
        result = subprocess.run(['node', '--test', str(regression)], text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_darwin_start_identity_layout_includes_microseconds(self):
        self.assertEqual(ctypes.sizeof(smoke.ProcBsdInfo), 136)
        self.assertEqual(smoke.ProcBsdInfo.start_usec.offset, 128)


class IsolationPreparationTests(unittest.TestCase):
    def test_gtk_preparation_is_portable_but_runtime_still_requires_linux(self):
        path=Path(__file__).with_name('tauri-webkit-smoke.py')
        spec=importlib.util.spec_from_file_location('gtk_portable_preparation',path)
        gtk=importlib.util.module_from_spec(spec);spec.loader.exec_module(gtk)
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);harness=root/'harness';evidence=root/'evidence'
            argv=[str(path),'--isolation-consumer','--harness-dir',str(harness),'--evidence-dir',str(evidence)]
            with mock.patch.object(gtk.sys,'platform','darwin'), mock.patch.object(gtk.sys,'argv',argv+['--prepare-only']), mock.patch.object(gtk.subprocess,'run',side_effect=AssertionError('preparation must not launch a process')):
                gtk.main()
            self.assertFalse(json.loads((evidence/'preparation.json').read_text())['runtimeVerified'])
            with mock.patch.object(gtk.sys,'platform','darwin'), mock.patch.object(gtk.sys,'argv',argv), self.assertRaisesRegex(RuntimeError,'actual WebKitGTK execution requires Linux'):
                gtk.main()

    def test_both_native_harnesses_prepare_roles_effect_guards_and_cleanup_without_gui(self):
        repo=Path(__file__).resolve().parents[2]
        original=(repo/'crates/splitty-tauri/src/client_ipc/provider.rs').read_bytes()
        for script in ['tauri-webkit-smoke.py','tauri-wkwebview-smoke.py']:
            with self.subTest(script=script),tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary);harness=root/'harness';evidence=root/'evidence'
                result=subprocess.run(['python3',str(repo/'scripts/testing'/script),'--prepare-only','--isolation-consumer','--harness-dir',str(harness),'--evidence-dir',str(evidence)],text=True,capture_output=True)
                self.assertEqual(result.returncode,0,result.stderr)
                main=(harness/'src/main.rs').read_text();provider=(harness/'src/client_ipc/provider.rs').read_text()
                self.assertIn('catalog::Role::Remote',provider)
                self.assertIn('negotiation::host_ready',provider)
                self.assertIn('let _ = client.finish().await',provider)
                self.assertIn('["fixture-one", "fixture-two"].contains(&target)',main)
                self.assertIn('smoke_finish_isolation(&app).await?',main)
                self.assertIn('backend.close().await',main)
                self.assertIn('smoke_pressure_file(first)',main)
                self.assertIn('if webview.label()=="main" { smoke_event("confirm")',main)
                self.assertTrue((harness/'src/remote.rs').is_symlink(),'production remote window builder remains intact')
                self.assertTrue((harness/'src/client_ipc/bootstrap.js').is_symlink(),'production private bootstrap remains intact')
                for name,authorization in [('editor','native.main_action(&webview, &auth)?'),('navigation','navigation::Decision')]:
                    source=(harness/'src'/f'{name}.rs').read_text()
                    self.assertNotIn('.opener().open_url',source)
                    self.assertIn('smoke_external_effect',source)
                self.assertIn('native.main_action(&webview, &auth)?',(harness/'src/editor.rs').read_text())
                self.assertTrue((evidence/'backend-substitution.json').is_file())
        self.assertEqual(original,(repo/'crates/splitty-tauri/src/client_ipc/provider.rs').read_bytes())
    def test_mode_does_not_duplicate_full_ui_or_add_a_scheduling_gate(self):
        root=Path(__file__).parent
        entry=(root/'tauri-webkit-smoke/entry.ts').read_text()
        consumer=(root/'tauri-webkit-smoke/consumer.ts').read_text()
        self.assertIn('fullUi: __SPLITTY_FULL_UI__ && !__SPLITTY_ISOLATION_CONSUMER__',entry)
        self.assertIn("observePressure } from '../tauri-windows-native-smoke/pressure.mjs'",consumer)
        self.assertNotIn('apiRepliesWhileStalled>',consumer)
        self.assertNotIn('fileCompletedBeforeSlowClose)',consumer)
        regression=subprocess.run(['node','--test',str(root/'tauri-windows-native-smoke/pressure.test.mjs')],capture_output=True,text=True)
        self.assertEqual(regression.returncode,0,regression.stdout+regression.stderr)

if __name__ == '__main__':
    unittest.main()
