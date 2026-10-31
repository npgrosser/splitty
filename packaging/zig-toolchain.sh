#!/usr/bin/env sh

# Provides the zig and cargo-zigbuild that cross-build the macOS servers on
# Linux, without an Apple SDK (openspec change remote-macos-hosts, D1): zig
# pinned by version and SHA-256, cargo-zigbuild by version, both kept in
# ~/.cache/splitty/zig-toolchain (or $SPLITTY_ZIG_TOOLCHAIN_DIR).
#
# Prints the directories to put first in PATH, separated by ':'.

set -eu

zig_version=0.14.1
zigbuild_version=0.23.4

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)
        platform=x86_64-linux
        zig_sha256=24aeeec8af16c381934a6cd7d95c807a8cb2cf7df9fa40d359aa884195c4716c
        ;;
    *)
        echo "the macOS servers are cross-built on x86_64 Linux only" >&2
        exit 1
        ;;
esac

for required_command in curl tar xz sha256sum cargo mktemp; do
    if ! command -v "$required_command" >/dev/null 2>&1; then
        echo "missing command for the zig toolchain: $required_command" >&2
        exit 1
    fi
done

toolchain_dir=${SPLITTY_ZIG_TOOLCHAIN_DIR:-$HOME/.cache/splitty/zig-toolchain}
zig_name=zig-$platform-$zig_version
zig_dir=$toolchain_dir/$zig_name
mkdir -p "$toolchain_dir"

if [ ! -x "$zig_dir/zig" ]; then
    download=$(mktemp -d "$toolchain_dir/.download.XXXXXX")
    trap 'rm -rf "$download"' EXIT HUP INT TERM
    echo "Downloading zig $zig_version" >&2
    curl -sSfL -o "$download/zig.tar.xz" "https://ziglang.org/download/$zig_version/$zig_name.tar.xz"
    if ! printf '%s  %s\n' "$zig_sha256" "$download/zig.tar.xz" | sha256sum -c - >/dev/null 2>&1; then
        echo "the zig download does not match its pinned checksum" >&2
        exit 1
    fi
    tar -xJf "$download/zig.tar.xz" -C "$download"
    rm -rf "$zig_dir"
    mv "$download/$zig_name" "$zig_dir"
fi

if ! "$toolchain_dir/bin/cargo-zigbuild" --version 2>/dev/null | grep -qx "cargo-zigbuild $zigbuild_version"; then
    echo "Installing cargo-zigbuild $zigbuild_version" >&2
    cargo install --locked --quiet --version "$zigbuild_version" --root "$toolchain_dir" cargo-zigbuild >&2
fi

printf '%s:%s\n' "$toolchain_dir/bin" "$zig_dir"
