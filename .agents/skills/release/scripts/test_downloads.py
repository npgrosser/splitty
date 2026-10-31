import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import downloads


class HostDownloadsTests(unittest.TestCase):
    def test_wsl_targets_windows_and_resolves_redirected_downloads(self):
        with patch.dict(os.environ, {"WSL_DISTRO_NAME": "Ubuntu"}, clear=True), \
             patch("platform.system", return_value="Linux"), \
             patch("platform.release", return_value="6.1"):
            self.assertEqual(downloads.host_platform(), ("windows", "wsl"))
        with patch.object(downloads, "command", side_effect=[r"D:\Users\O'Brien\Downloads", "/mnt/d/Users/O'Brien/Downloads"]) as run:
            path, native = downloads.downloads_folder("windows", "wsl")
            self.assertEqual(path, Path("/mnt/d/Users/O'Brien/Downloads"))
            self.assertEqual(native, r"D:\Users\O'Brien\Downloads")
            self.assertEqual(run.call_args.args[0], ["wslpath", "-u", native])

    def test_failed_windows_lookup_never_falls_back_to_linux_home(self):
        with patch.object(downloads, "command", return_value=""):
            with self.assertRaises(RuntimeError):
                downloads.downloads_folder("windows", "wsl")

    def test_native_platforms_and_xdg_directory(self):
        for system, expected in [("Windows", "windows"), ("Darwin", "macos"), ("Linux", "linux")]:
            with patch.dict(os.environ, {}, clear=True), patch("platform.system", return_value=system), patch("platform.release", return_value="6.1"):
                self.assertEqual(downloads.host_platform(), (expected, "native"))
        with patch.object(downloads.shutil, "which", return_value="/usr/bin/xdg-user-dir"), \
             patch.object(downloads, "command", return_value="/home/test/Heruntergeladen"):
            self.assertEqual(downloads.downloads_folder("linux", "native")[0], Path("/home/test/Heruntergeladen"))
        with patch.object(downloads.Path, "home", return_value=Path("/Users/test")):
            self.assertEqual(downloads.downloads_folder("macos", "native")[0], Path("/Users/test/Downloads"))

    def test_copy_preserves_bytes_permissions_and_existing_downloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            destination = root / "Downloads"
            destination.mkdir()
            source = root / "Splitty 1.2.3.AppImage"
            source.write_bytes(b"fixture package\x00\xff")
            source.chmod(0o755)
            result = downloads.copy_artifact(source, destination)
            target = Path(result["path"])
            self.assertEqual(target.read_bytes(), source.read_bytes())
            self.assertEqual(target.stat().st_mode & 0o777, source.stat().st_mode & 0o777)
            self.assertEqual(Path(result["checksumFile"]).read_text(), f"{downloads.digest(source)}  {source.name}\n")
            self.assertEqual(downloads.copy_artifact(source, destination), result)
            source.write_bytes(b"different rebuild")
            with self.assertRaises(RuntimeError):
                downloads.copy_artifact(source, destination)
            self.assertEqual(target.read_bytes(), b"fixture package\x00\xff")

    def test_xdg_config_is_read_as_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            config = Path(temporary) / "user-dirs.dirs"
            config.write_text('XDG_DOWNLOAD_DIR="$HOME/Downloads with spaces"\n')
            with patch.dict(os.environ, {"XDG_CONFIG_HOME": temporary}), patch.object(downloads.shutil, "which", return_value=None):
                self.assertEqual(downloads.downloads_folder("linux", "native")[0], Path.home() / "Downloads with spaces")
                config.write_text('XDG_DOWNLOAD_DIR="$(touch /tmp/never-execute)"\n')
                with self.assertRaises(RuntimeError):
                    downloads.downloads_folder("linux", "native")


if __name__ == "__main__":
    unittest.main()
