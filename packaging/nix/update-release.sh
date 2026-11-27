#!/usr/bin/env bash

# Points packaging/nix/release.json at a published release: its version and
# the checksums of the files the flake fetches (the Linux tar.gz for
# `splitty-bin`, the four server archives for `splitty`), taken from the
# release's SHA256SUMS.
#
#   packaging/nix/update-release.sh <version>
#
# Run it once the release is published; before that the files do not exist.

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: packaging/nix/update-release.sh <version>" >&2
    exit 2
fi
version=${1#v}
here=$(cd "$(dirname "$0")" && pwd -P)
sums=$(curl --fail --silent --show-error --location \
    "https://github.com/npgrosser/splitty/releases/download/v$version/SHA256SUMS")

files=(
    "splitty-$version-linux-x86_64.tar.gz"
    splitty-darwin-aarch64.tar.gz
    splitty-darwin-x86_64.tar.gz
    splitty-wsl-aarch64.tar.gz
    splitty-wsl-x86_64.tar.gz
)

{
    printf '{\n  "version": "%s",\n  "sha256": {\n' "$version"
    last=$((${#files[@]} - 1))
    for index in "${!files[@]}"; do
        file=${files[$index]}
        hash=$(awk -v file="$file" '$2 == file || $2 == "*" file { print $1 }' <<<"$sums")
        if ! [[ "$hash" =~ ^[0-9a-f]{64}$ ]]; then
            echo "SHA256SUMS of v$version has no checksum for $file" >&2
            exit 1
        fi
        separator=,
        [ "$index" -eq "$last" ] && separator=
        printf '    "%s": "%s"%s\n' "$file" "$hash" "$separator"
    done
    printf '  }\n}\n'
} > "$here/release.json.new"
mv "$here/release.json.new" "$here/release.json"
echo "packaging/nix/release.json names v$version"
