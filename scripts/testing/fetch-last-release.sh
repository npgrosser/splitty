#!/usr/bin/env bash
set -euo pipefail

# Fetches the `splitty` command of the last release for Linux x86_64 and checks
# it against the release's SHA256SUMS before anything runs it. The version-skew
# smoke test (crates/splitty-server/tests/version_skew.rs) runs this build next
# to the current one.
#
# The last release is the newest release that is not a prerelease, as GitHub's
# "latest release" names it; --tag picks another one. The asset is the static
# x86_64 server and CLI that release.yml publishes as splitty-wsl-x86_64.tar.gz
# (the same `splitty` the Linux packages ship).
#
# Usage:
#   scripts/testing/fetch-last-release.sh --out <dir> [--repo <owner/name>] [--tag <tag>]
#
# <dir> must not exist or be empty. On success the script prints two lines,
# for `>> "$GITHUB_ENV"` or `export`:
#   SPLITTY_TEST_RELEASED_BINARY=<absolute path of the verified splitty>
#   SPLITTY_TEST_RELEASED_VERSION=<its version>
# Needs gh (logged in, or GH_TOKEN set), tar and sha256sum.

ASSET=splitty-wsl-x86_64.tar.gz

usage() {
    echo "Usage: $0 --out <dir> [--repo <owner/name>] [--tag <tag>]" >&2
}

fail() {
    echo "error: $*" >&2
    exit 1
}

out=
repo=${GITHUB_REPOSITORY:-}
tag=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --out) out=${2:?}; shift 2 ;;
        --repo) repo=${2:?}; shift 2 ;;
        --tag) tag=${2:?}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done
[ -n "$out" ] || { usage; exit 2; }

[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] \
    || fail "the released build runs on Linux x86_64 only, not $(uname -sm)"
if [ -z "$repo" ]; then
    repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner) \
        || fail "no --repo given and no GitHub repository found for this checkout"
fi
if [ -z "$tag" ]; then
    tag=$(gh api "repos/$repo/releases/latest" --jq .tag_name) \
        || fail "cannot find the latest release of $repo"
fi
version=${tag#v}
[[ "$tag" != "$version" && "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] \
    || fail "'$tag' is not a release tag"

if [ -e "$out" ] && [ -n "$(find "$out" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
    fail "$out is not empty"
fi
mkdir -p "$out/download" "$out/$tag"
out=$(cd "$out" && pwd -P)

echo "Fetching $ASSET and SHA256SUMS of $repo $tag" >&2
gh release download "$tag" --repo "$repo" \
    --pattern "$ASSET" --pattern SHA256SUMS --dir "$out/download" >&2

# Exactly one line names the asset; sha256sum checks the file against it.
(
    cd "$out/download"
    lines=$(awk -v name="$ASSET" '$2 == name || $2 == "*" name' SHA256SUMS)
    [ -n "$lines" ] && [ "$(printf '%s\n' "$lines" | wc -l)" -eq 1 ] \
        || fail "SHA256SUMS of $tag does not name $ASSET exactly once"
    printf '%s\n' "$lines" | sha256sum --check --strict - >&2
) || fail "$ASSET of $tag does not match its SHA256SUMS"

tar -xzf "$out/download/$ASSET" -C "$out/$tag" --no-same-owner
binary="$out/$tag/splitty"
[ -f "$binary" ] && [ ! -L "$binary" ] && [ -x "$binary" ] \
    || fail "$ASSET of $tag holds no executable splitty"
# Without the caller's environment: inside a Splitty pane it names that
# pane's daemon.
reported=$(env -i PATH=/usr/bin:/bin "$binary" --version)
[ "$reported" = "splitty $version" ] \
    || fail "the splitty of $tag reports '$reported'"

echo "Verified $reported at $binary" >&2
echo "SPLITTY_TEST_RELEASED_BINARY=$binary"
echo "SPLITTY_TEST_RELEASED_VERSION=$version"
