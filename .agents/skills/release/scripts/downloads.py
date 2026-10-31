#!/usr/bin/env python3
"""Locate host Downloads and copy explicit release artifacts without clobbering."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys


def command(argv):
    return subprocess.check_output(argv, encoding="utf-8").strip().lstrip("\ufeff")


def host_platform():
    system = platform.system()
    if system == "Windows":
        return "windows", "native"
    if system == "Darwin":
        return "macos", "native"
    if system == "Linux":
        wsl = bool(os.environ.get("WSL_INTEROP") or os.environ.get("WSL_DISTRO_NAME"))
        wsl = wsl or "microsoft" in platform.release().lower()
        return ("windows", "wsl") if wsl else ("linux", "native")
    raise RuntimeError(f"Unsupported host: {system}")


def downloads_folder(host, environment):
    if host == "windows":
        native = command([
            "powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); "
            "$folder = (New-Object -ComObject Shell.Application).NameSpace('shell:Downloads'); "
            "if ($null -eq $folder) { throw 'Downloads folder unavailable' }; $folder.Self.Path",
        ])
        if not re.match(r"^(?:[A-Za-z]:\\|\\\\)", native):
            raise RuntimeError("Windows did not return an absolute Downloads path")
        local = command(["wslpath", "-u", native]) if environment == "wsl" else native
        return Path(local), native
    if host == "macos":
        path = Path.home() / "Downloads"
    elif shutil.which("xdg-user-dir"):
        configured = command(["xdg-user-dir", "DOWNLOAD"])
        if not configured:
            raise RuntimeError("xdg-user-dir returned an empty Downloads path")
        path = Path(configured)
    else:
        path = Path.home() / "Downloads"
        config = Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "user-dirs.dirs"
        if config.is_file():
            match = re.search(r'^XDG_DOWNLOAD_DIR="([^"\n]+)"\s*$', config.read_text(), re.M)
            if match:
                value = match.group(1).replace("${HOME}", str(Path.home())).replace("$HOME", str(Path.home()))
                if "$" in value or "`" in value:
                    raise RuntimeError("Unsupported XDG Downloads expression; use xdg-user-dir")
                path = Path(value)
    return path, str(path)


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def copy_artifact(source, destination):
    if not source.is_file() or source.stat().st_size == 0:
        raise RuntimeError(f"Artifact is missing or empty: {source}")
    if any(char in source.name for char in "\r\n\\"):
        raise RuntimeError("Artifact name cannot be represented in a SHA-256 sidecar")
    checksum = digest(source)
    target = destination / source.name
    sidecar = target.with_name(target.name + ".sha256")
    checksum_line = f"{checksum}  {target.name}\n"
    if target.is_symlink() or sidecar.is_symlink():
        raise RuntimeError(f"Refusing a symlink at the destination: {target}")
    if target.exists() and (not target.is_file() or digest(target) != checksum):
        raise RuntimeError(f"A different download already exists: {target}")
    if sidecar.exists() and (not sidecar.is_file() or sidecar.read_text() != checksum_line):
        raise RuntimeError(f"A different checksum already exists: {sidecar}")
    if not target.exists():
        # Exclusive creation also catches another process claiming the name.
        with target.open("xb") as output:
            try:
                with source.open("rb") as artifact:
                    shutil.copyfileobj(artifact, output)
                output.flush()
            except BaseException:
                target.unlink(missing_ok=True)
                raise
        if digest(target) != checksum:
            target.unlink()
            raise RuntimeError("Artifact changed during copying")
        shutil.copymode(source, target)
    if not sidecar.exists():
        with sidecar.open("x", encoding="utf-8", newline="\n") as output:
            output.write(checksum_line)
    return {"source": str(source), "path": str(target), "sha256": checksum, "checksumFile": str(sidecar)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--copy", nargs="+", metavar="ARTIFACT", type=Path)
    args = parser.parse_args()
    host, environment = host_platform()
    directory, native = downloads_folder(host, environment)
    if not directory.is_absolute():
        raise RuntimeError(f"Downloads is not absolute: {directory}")
    report = {"platform": host, "environment": environment, "architecture": platform.machine(),
              "downloads": str(directory), "nativeDownloads": native, "artifacts": []}
    if args.copy:
        directory.mkdir(parents=True, exist_ok=True)
        for source in args.copy:
            artifact = copy_artifact(source.resolve(), directory)
            artifact["nativePath"] = command(["wslpath", "-w", artifact["path"]]) if environment == "wsl" else artifact["path"]
            report["artifacts"].append(artifact)
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
