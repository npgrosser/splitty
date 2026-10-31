#!/usr/bin/env bash
set -euo pipefail

# Splitty has one version for everything it ships: the workspace version in
# Cargo.toml, which the Tauri app, the Electron app and Cargo.lock repeat.
# Hosts and the Windows WSL backend install a server per version, so every
# release needs a version of its own, or clients keep an older server of the
# same version.
#
# Usage:
#   scripts/release.sh <version>        Set the version everywhere, commit, tag v<version>
#   scripts/release.sh --check <tag>    Fail unless every version file says the tag's version
#
# The release workflow runs --check for a pushed tag; it builds the installers
# and the packages and publishes the GitHub release.

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_TOML="$REPO_ROOT/Cargo.toml"
CARGO_LOCK="$REPO_ROOT/Cargo.lock"
TAURI_CONF="$REPO_ROOT/crates/splitty-tauri/tauri.conf.json"
ELECTRON_PACKAGE="$REPO_ROOT/frontend-electron/package.json"

usage() {
    echo "Usage: $0 <version>"
    echo "       $0 --check <tag>"
    echo ""
    echo "<version> is MAJOR.MINOR.PATCH, optionally with a pre-release (0.2.0-rc.1)."
}

is_version() {
    [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$ ]]
}

cargo_version() {
    sed -n '/^\[workspace\.package\]/,/^\[/ s/^version = "\(.*\)"$/\1/p' "$CARGO_TOML"
}

# The top-level "version" of a JSON file indented with two spaces.
json_version() {
    sed -n 's/^  "version": "\(.*\)",$/\1/p' "$1" | head -n 1
}

# The versions Cargo.lock records for the workspace's crates, one per line.
lock_versions() {
    awk '/^name = "splitty-/ { getline; sub(/^version = "/, ""); sub(/"$/, ""); print }' "$CARGO_LOCK" | sort -u
}

check() {
    local tag=$1
    local version=${tag#v}
    if [ "$tag" = "$version" ] || ! is_version "$version"; then
        echo "error: '$tag' is not a release tag (v<version>, for example v0.2.0)" >&2
        return 1
    fi

    local failed=0
    local name actual
    for name in Cargo.toml tauri.conf.json package.json Cargo.lock; do
        case "$name" in
            Cargo.toml) actual=$(cargo_version) ;;
            tauri.conf.json) actual=$(json_version "$TAURI_CONF") ;;
            package.json) actual=$(json_version "$ELECTRON_PACKAGE") ;;
            Cargo.lock) actual=$(lock_versions | paste -sd ' ' -) ;;
        esac
        if [ "$actual" != "$version" ]; then
            echo "error: $name says '${actual:-nothing}', the tag $tag says $version" >&2
            failed=1
        fi
    done
    if [ "$failed" -ne 0 ]; then
        echo "Set the version with: scripts/release.sh $version" >&2
        return 1
    fi
    echo "Every version file says $version."
}

bump() {
    local version=$1
    local tag="v$version"
    if ! is_version "$version"; then
        echo "error: '$version' is not a version (MAJOR.MINOR.PATCH[-pre-release])" >&2
        exit 2
    fi

    cd "$REPO_ROOT"
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "error: the working tree has changes; commit or stash them first" >&2
        exit 1
    fi
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
        echo "error: the tag $tag exists already" >&2
        exit 1
    fi

    local current
    current=$(cargo_version)
    if [ "$current" = "$version" ]; then
        echo "error: the version is $version already" >&2
        exit 1
    fi

    # Read before writing, preserving file permissions. BSD sed (macOS) has
    # neither GNU sed's `-i` syntax nor its `0,/pattern/` address.
    local updated file
    updated=$(sed "/^\[workspace\.package\]/,/^\[/ s/^version = \".*\"$/version = \"$version\"/" "$CARGO_TOML")
    printf '%s\n' "$updated" > "$CARGO_TOML"
    for file in "$TAURI_CONF" "$ELECTRON_PACKAGE"; do
        updated=$(sed "s/^  \"version\": \".*\",$/  \"version\": \"$version\",/" "$file")
        printf '%s\n' "$updated" > "$file"
    done
    # Records the new version of the workspace's crates, nothing else.
    cargo update --workspace --offline --quiet

    check "$tag"

    git add -- "$CARGO_TOML" "$CARGO_LOCK" "$TAURI_CONF" "$ELECTRON_PACKAGE"
    git commit --quiet -m "chore(release): $tag"
    git tag -a "$tag" -m "Splitty $tag"

    echo ""
    echo "Committed and tagged $tag (was $current). Push both to build the release:"
    echo "  git push --atomic origin HEAD $tag"
}

case "${1:-}" in
    --check)
        [ "$#" -eq 2 ] || { usage >&2; exit 2; }
        check "$2"
        ;;
    -h|--help)
        usage
        ;;
    "")
        usage >&2
        exit 2
        ;;
    *)
        [ "$#" -eq 1 ] || { usage >&2; exit 2; }
        bump "$1"
        ;;
esac
