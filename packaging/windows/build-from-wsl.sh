#!/usr/bin/env sh

set -eu

repo_root=$(CDPATH= cd "$(dirname "$0")/../.." && pwd -P)

case "$repo_root" in
    /mnt/[a-zA-Z]/*) ;;
    *)
        echo "The full Windows installer must be built from a checkout on a Windows drive." >&2
        echo "Clone Splitty below /mnt/c (for example /mnt/c/Code/splitty), then run this script there." >&2
        exit 1
        ;;
esac

windows_script=$(wslpath -w "$repo_root/packaging/windows/build.ps1")
distro=${WSL_DISTRO_NAME:-}

if [ -n "$distro" ]; then
    exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$windows_script" -Distro "$distro" "$@"
fi

exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$windows_script" "$@"
