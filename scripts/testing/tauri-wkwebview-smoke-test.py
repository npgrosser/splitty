#!/usr/bin/env python3
"""Portable observer/preparation checks; these never claim WKWebView coverage."""
import ctypes
import importlib.util
from pathlib import Path
import unittest

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
        self.assertIn('msg_send![NSApp(), windows]', result)
        self.assertIn('windows.objectAtIndex(index).isVisible()', result)
        self.assertEqual(source[:source.index('async fn smoke_windows')], result[:result.index('async fn smoke_windows')])

    def test_darwin_start_identity_layout_includes_microseconds(self):
        self.assertEqual(ctypes.sizeof(smoke.ProcBsdInfo), 136)
        self.assertEqual(smoke.ProcBsdInfo.start_usec.offset, 128)


if __name__ == '__main__':
    unittest.main()
