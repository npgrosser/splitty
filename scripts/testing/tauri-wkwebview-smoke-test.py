#!/usr/bin/env python3
"""Portable observer/preparation checks; these never claim WKWebView coverage."""
import ctypes
import importlib.util
from pathlib import Path
import unittest
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


if __name__ == '__main__':
    unittest.main()
