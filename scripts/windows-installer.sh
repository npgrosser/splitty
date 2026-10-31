#!/usr/bin/env bash
set -euo pipefail

# Build the Windows installer on this PC, from WSL, in a few minutes instead
# of waiting for the release workflow: bring a checkout on a Windows drive to
# this repository's HEAD and run packaging/windows/build.ps1 there (the
# Windows Rust toolchain, Node and pnpm; the Linux payload in this WSL
# distribution). Uncommitted changes are not built.
#
# Usage:
#   scripts/windows-installer.sh [--skip-tests] [--install]
#     --skip-tests  skip the frontend type check and the Windows unit tests
#     --install     start the built setup afterwards
#
# Environment:
#   SPLITTY_WINDOWS_CHECKOUT  the Windows checkout, as a Windows path
#                             (default: %USERPROFILE%\Code\splitty-windows-preview);
#                             cloned from this repository when it does not exist
#   SPLITTY_WINDOWS_PATH      extra Windows PATH entries for the build, separated
#                             by ';' (for example where pnpm is installed)

skip_tests=0
install=0
for arg in "$@"; do
    case "$arg" in
        --skip-tests) skip_tests=1 ;;
        --install) install=1 ;;
        *) echo "usage: $0 [--skip-tests] [--install]" >&2; exit 2 ;;
    esac
done

if [ -z "${WSL_DISTRO_NAME:-}" ] || ! command -v powershell.exe >/dev/null; then
    echo "error: run this inside WSL on the Windows PC that builds the installer" >&2
    exit 1
fi

repo="$(cd "$(dirname "$0")/.." && pwd)"
profile="$(powershell.exe -NoProfile -Command '$env:USERPROFILE' | tr -d '\r')"
checkout_win="${SPLITTY_WINDOWS_CHECKOUT:-$profile\\Code\\splitty-windows-preview}"
checkout="$(wslpath -u "$checkout_win")"

if [ -n "$(git -C "$repo" status --porcelain)" ]; then
    echo "note: uncommitted changes in $repo are not part of the build" >&2
fi

if [ ! -d "$checkout/.git" ]; then
    echo "Cloning $repo to $checkout_win"
    git clone --quiet "$repo" "$checkout"
fi
if [ -n "$(git -C "$checkout" status --porcelain)" ]; then
    echo "error: the Windows checkout $checkout_win has local changes; commit or discard them first" >&2
    exit 1
fi
git -C "$checkout" fetch --quiet --tags "$repo" HEAD
git -C "$checkout" checkout --quiet --detach FETCH_HEAD
echo "Building $(git -C "$checkout" log --oneline -1) in $checkout_win"

build_args="-Distro '$WSL_DISTRO_NAME'"
[ "$skip_tests" = 1 ] && build_args="$build_args -SkipTests"
script="\$ErrorActionPreference = 'Stop'
\$env:Path = \"${SPLITTY_WINDOWS_PATH:-};\$env:USERPROFILE\\.cargo\\bin;\" + \$env:Path
& '$checkout_win\\packaging\\windows\\build.ps1' $build_args"
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "$script"

bundle="$checkout/target/x86_64-pc-windows-msvc/release/bundle/nsis"
setup="$(ls -t "$bundle"/*-setup.exe | head -n 1)"
echo
echo "Setup: $(wslpath -w "$setup")"
if [ "$install" = 1 ]; then
    powershell.exe -NoProfile -Command "Start-Process -FilePath '$(wslpath -w "$setup")'"
fi
