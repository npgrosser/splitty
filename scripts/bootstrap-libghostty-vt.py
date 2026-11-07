#!/usr/bin/env python3
"""Explicit, hash-pinned local build of libghostty-vt for splitty-gpui."""

import argparse
import hashlib
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from urllib.request import Request, urlopen
import zipfile


VERSION = "1.0.0"
SOURCE_SHA256 = "4a6ef758be721923e1969e7b81b443dcc92d8dd3fb5a39600c03a8817a753676"
DEPS_SHA256 = "4d5ea5e448e4c7ea8fb55ccbaa8668f2a97d44f438cee831e2b7ecd162d85c29"
ZIG_VERSION = "0.17.0"
ZIG_ARCHIVES = {
    "x86_64-linux": ("tar.xz", "1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026"),
    "aarch64-linux": ("tar.xz", "9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8"),
    "x86_64-macos": ("tar.xz", "4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6"),
    "aarch64-macos": ("tar.xz", "b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a"),
    "x86_64-windows": ("zip", "b5663f69581dcf391293fbf16c06cb80d81d806545ce618b4d0bab7f0eb8c428"),
}
RELEASE_BASE = f"https://release.files.ghostty.org/libghostty-vt/{VERSION}"


def host_target():
    architecture = platform.machine().lower()
    architecture = {"amd64": "x86_64", "arm64": "aarch64"}.get(architecture, architecture)
    system = {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}.get(platform.system())
    target = f"{architecture}-{system}"
    if target not in ZIG_ARCHIVES:
        raise RuntimeError(f"unsupported native libghostty-vt build host: {target}")
    return target


def default_prefix(target):
    if sys.platform == "win32":
        cache = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local")) / "Splitty" / "Cache"
    elif sys.platform == "darwin":
        cache = Path.home() / "Library" / "Caches" / "splitty"
    else:
        cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "splitty"
    return cache / "libghostty-vt" / VERSION / target


def download(url, destination, expected_hash):
    digest = hashlib.sha256()
    print(f"Downloading {url}", file=sys.stderr)
    request = Request(url, headers={"User-Agent": "Splitty-native-dependency-bootstrap/1"})
    with urlopen(request, timeout=60) as response, destination.open("wb") as output:
        while chunk := response.read(1024 * 1024):
            digest.update(chunk)
            output.write(chunk)
    actual = digest.hexdigest()
    if actual != expected_hash:
        raise RuntimeError(f"SHA-256 mismatch for {url}: expected {expected_hash}, got {actual}")


def checked_destination(root, name):
    if "\\" in name:
        raise RuntimeError(f"unsafe archive member: {name}")
    parts = PurePosixPath(name).parts
    if not parts or any(part in ("..", "") or ":" in part for part in parts) or name.startswith("/"):
        raise RuntimeError(f"unsafe archive member: {name}")
    return root.joinpath(*parts)


def extract_tar(archive, root):
    with tarfile.open(archive, "r:*") as source:
        for member in source:
            destination = checked_destination(root, member.name)
            if member.isdir():
                destination.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                destination.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as content, destination.open("wb") as output:
                    shutil.copyfileobj(content, output)
                destination.chmod(member.mode & 0o777)
            else:
                raise RuntimeError(f"unsupported archive member: {member.name}")


def extract_zip(archive, root):
    with zipfile.ZipFile(archive) as source:
        for member in source.infolist():
            destination = checked_destination(root, member.filename)
            if stat.S_ISLNK(member.external_attr >> 16):
                raise RuntimeError(f"unsupported archive member: {member.filename}")
            if member.is_dir():
                destination.mkdir(parents=True, exist_ok=True)
            else:
                destination.parent.mkdir(parents=True, exist_ok=True)
                with source.open(member) as content, destination.open("wb") as output:
                    shutil.copyfileobj(content, output)


def verify_prefix(prefix, target):
    archive = "ghostty-vt-static.lib" if target.endswith("windows") else "libghostty-vt.a"
    version_header = prefix / "include" / "ghostty" / "vt" / "version.h"
    marker = prefix / ".splitty-ghostty-vt-build"
    expected_marker = "\n".join((VERSION, target, SOURCE_SHA256, DEPS_SHA256, ZIG_ARCHIVES[target][1], ""))
    return (
        marker.is_file()
        and marker.read_text(encoding="ascii") == expected_marker
        and (prefix / "lib" / archive).is_file()
        and version_header.is_file()
        and f'#define GHOSTTY_VT_VERSION "{VERSION}"' in version_header.read_text(encoding="utf-8")
    )


def check_static_link(prefix):
    source = Path(__file__).resolve().parent / "testing" / "ghostty-vt-link.rs"
    with tempfile.TemporaryDirectory(prefix="ghostty-vt-link-") as temporary:
        work = Path(temporary)
        archive = "ghostty-vt-static.lib" if sys.platform == "win32" else "libghostty-vt.a"
        static_dir = work / "native-static"
        static_dir.mkdir()
        shutil.copy2(prefix / "lib" / archive, static_dir / archive)
        executable = work / ("ghostty-vt-link.exe" if sys.platform == "win32" else "ghostty-vt-link")
        subprocess.run(
            ["rustc", "--edition=2021", "-L", f"native={static_dir}", "-o", str(executable), str(source)],
            check=True,
        )
        if sys.platform == "darwin":
            linked = subprocess.run(["otool", "-L", str(executable)], check=True, capture_output=True, text=True)
            if any("libghostty-vt" in line and ".dylib" in line for line in linked.stdout.splitlines()):
                raise RuntimeError("Rust static-link smoke unexpectedly depends on libghostty-vt.dylib")
        subprocess.run([str(executable)], check=True)
    print("Pinned static library linked and ran from Rust", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", type=Path, help="absolute install prefix (defaults to the user cache)")
    parser.add_argument("--check", action="store_true", help="compile and run a Rust static-link smoke check")
    args = parser.parse_args()
    target = host_target()
    prefix = args.prefix or default_prefix(target)
    if not prefix.is_absolute():
        parser.error("--prefix must be an absolute path")
    prefix = prefix.resolve(strict=False)
    if prefix.exists() or prefix.is_symlink():
        if verify_prefix(prefix, target):
            if args.check:
                check_static_link(prefix)
            print(prefix)
            return
        raise RuntimeError(f"{prefix} exists but is not a matching verified build; choose another prefix")

    prefix.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ghostty-vt-build-", dir=prefix.parent) as temporary:
        work = Path(temporary)
        source_archive = work / f"libghostty-vt-{VERSION}.tar.gz"
        deps_archive = work / f"libghostty-vt-{VERSION}-deps.tar.gz"
        zig_extension, zig_hash = ZIG_ARCHIVES[target]
        zig_name = f"zig-{target}-{ZIG_VERSION}.{zig_extension}"
        zig_archive = work / zig_name
        download(f"{RELEASE_BASE}/{source_archive.name}", source_archive, SOURCE_SHA256)
        download(f"{RELEASE_BASE}/{deps_archive.name}", deps_archive, DEPS_SHA256)
        download(f"https://ziglang.org/download/{ZIG_VERSION}/{zig_name}", zig_archive, zig_hash)
        extract_tar(source_archive, work)
        extract_tar(deps_archive, work)
        if zig_extension == "zip":
            extract_zip(zig_archive, work)
        else:
            extract_tar(zig_archive, work)

        source = work / f"libghostty-vt-{VERSION}"
        dependencies = work / f"libghostty-vt-{VERSION}-deps"
        zig = work / f"zig-{target}-{ZIG_VERSION}" / ("zig.exe" if target.endswith("windows") else "zig")
        if not (source / "LIB_VERSION").is_file() or (source / "LIB_VERSION").read_text().strip() != VERSION:
            raise RuntimeError("release source is missing the expected LIB_VERSION")
        if not dependencies.is_dir() or not zig.is_file():
            raise RuntimeError("release dependencies or pinned Zig executable are missing")

        abi = Path(__file__).resolve().parent.parent / "crates" / "splitty-gpui" / "src" / "terminal_engine" / "abi.c"
        subprocess.run(
            [str(zig), "cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-c", "-I", str(source / "include"), str(abi), "-o", str(work / "ghostty-abi.o")],
            check=True,
        )

        environment = os.environ.copy()
        environment["ZIG_GLOBAL_CACHE_DIR"] = str(work / "zig-cache")
        command = [str(zig), "build", "--system", str(dependencies), "-Doptimize=ReleaseFast", "-Dcpu=baseline", "--prefix", str(prefix)]
        prefix.mkdir()
        try:
            print("Building pinned libghostty-vt", VERSION, "for", target, file=sys.stderr)
            subprocess.run(command, cwd=source, env=environment, check=True)
            marker = prefix / ".splitty-ghostty-vt-build"
            marker.write_text("\n".join((VERSION, target, SOURCE_SHA256, DEPS_SHA256, zig_hash, "")), encoding="ascii")
            if not verify_prefix(prefix, target):
                raise RuntimeError("libghostty-vt build did not install the expected headers and static library")
        except BaseException:
            shutil.rmtree(prefix)
            raise
    if args.check:
        check_static_link(prefix)
    print(prefix)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"libghostty-vt bootstrap failed: {error}", file=sys.stderr)
        sys.exit(1)
