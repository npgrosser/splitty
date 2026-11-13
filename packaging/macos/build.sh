#!/usr/bin/env sh

# Builds the Splitty macOS app locally as the release workflow does: the
# frontend, the servers the app installs on remote Linux and macOS hosts, and
# the ad-hoc signed Tauri app (tauri.macos.conf.json), as a DMG by default.
#
# The four servers come from SPLITTY_REMOTE_PAYLOAD_X86_64, _AARCH64,
# _DARWIN_X86_64 and _DARWIN_AARCH64 when all of them are set (a release run's
# artifacts, for example); otherwise packaging/windows/build-wsl-payload.sh
# builds them from this checkout into build/remote-payloads.

set -eu

usage() {
    echo "usage: $0 [--universal] [--skip-tests] [--bundles dmg|app|app,dmg]" >&2
    exit 2
}

universal=false
skip_tests=false
bundles=dmg
while [ "$#" -gt 0 ]; do
    case "$1" in
        --universal) universal=true ;;
        --skip-tests) skip_tests=true ;;
        --bundles)
            [ "$#" -ge 2 ] || usage
            bundles=$2
            shift
            ;;
        *) usage ;;
    esac
    shift
done

if [ "$(uname -s)" != Darwin ]; then
    echo "this builds the macOS app on a Mac; the README names the builds for other platforms" >&2
    exit 1
fi
for required_command in cargo rustup pnpm git xcrun shasum; do
    if ! command -v "$required_command" >/dev/null 2>&1; then
        echo "missing build command: $required_command" >&2
        exit 1
    fi
done
if ! xcrun --find cc >/dev/null 2>&1; then
    echo "missing Xcode's command line tools: xcode-select --install" >&2
    exit 1
fi
# Tauri's DMG script mounts its image as /Volumes/Splitty and fails when an
# opened Splitty DMG holds that name already.
case ",$bundles," in
    *,dmg,*)
        if [ -e /Volumes/Splitty ]; then
            echo "eject /Volumes/Splitty (an opened Splitty DMG) first; the DMG build mounts its image under that name" >&2
            exit 1
        fi
        ;;
esac

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
commit=$(git -C "$repo_root" rev-parse HEAD)
target_dir=${CARGO_TARGET_DIR:-$repo_root/target}
tauri=$repo_root/frontend/node_modules/.bin/tauri

echo "[1/4] Preparing frontend"
(
    cd "$repo_root/frontend"
    pnpm install --frozen-lockfile
    if [ "$skip_tests" = false ]; then
        pnpm check
    fi
    pnpm build
)

payload_variables="SPLITTY_REMOTE_PAYLOAD_X86_64 SPLITTY_REMOTE_PAYLOAD_AARCH64 SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64 SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64"
set_variables=0
for variable in $payload_variables; do
    if [ -n "$(printenv "$variable" || true)" ]; then
        set_variables=$((set_variables + 1))
    fi
done

if [ "$set_variables" -eq 4 ]; then
    echo "[2/4] Using the remote payloads named by SPLITTY_REMOTE_PAYLOAD_*"
    for variable in $payload_variables; do
        archive=$(printenv "$variable")
        if [ ! -f "$archive" ]; then
            echo "$variable names no file: $archive" >&2
            exit 1
        fi
    done
elif [ "$set_variables" -eq 0 ]; then
    echo "[2/4] Building remote payloads"
    payload_script=$repo_root/packaging/windows/build-wsl-payload.sh
    payload_dir=$repo_root/build/remote-payloads
    # aarch64 and macOS first, so the x86_64 server carries them as the
    # release's does.
    for arch in aarch64 darwin-aarch64 darwin-x86_64; do
        sh "$payload_script" "$repo_root" "$payload_dir" "$commit" "$arch"
    done
    SPLITTY_REMOTE_PAYLOAD_AARCH64=$payload_dir/splitty-wsl-aarch64.tar.gz
    SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64=$payload_dir/splitty-darwin-aarch64.tar.gz
    SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64=$payload_dir/splitty-darwin-x86_64.tar.gz
    export SPLITTY_REMOTE_PAYLOAD_AARCH64 SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64 SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64
    sh "$payload_script" "$repo_root" "$payload_dir" "$commit" x86_64
    SPLITTY_REMOTE_PAYLOAD_X86_64=$payload_dir/splitty-wsl-x86_64.tar.gz
    export SPLITTY_REMOTE_PAYLOAD_X86_64
else
    echo "set all four SPLITTY_REMOTE_PAYLOAD_* variables or none ($payload_variables)" >&2
    exit 2
fi

export SPLITTY_COMMIT="$commit"
export SPLITTY_BUILD_TIME=1970-01-01T00:00:00Z
export SOURCE_DATE_EPOCH=0

if [ "$skip_tests" = false ]; then
    echo "[3/4] Running desktop unit tests"
    (cd "$repo_root" && cargo test --locked --release --package splitty-gui --lib)
else
    echo "[3/4] Skipping desktop unit tests"
fi

if [ ! -x "$tauri" ]; then
    echo "the Tauri CLI was not installed at $tauri" >&2
    exit 1
fi
set -- build --bundles "$bundles"
if [ "$universal" = true ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    set -- "$@" --target universal-apple-darwin
    bundle_dir=$target_dir/universal-apple-darwin/release/bundle
else
    bundle_dir=$target_dir/release/bundle
fi

echo "[4/4] Building the app ($bundles)"
started=$(mktemp "${TMPDIR:-/tmp}/splitty-macos-build.XXXXXX")
trap 'rm -f "$started"' EXIT
(cd "$repo_root/crates/splitty-tauri" && "$tauri" "$@")

echo ""
found=false
for output in "$bundle_dir"/dmg/*.dmg "$bundle_dir"/macos/Splitty.app; do
    if [ -e "$output" ] && [ -n "$(find "$output" -prune -newer "$started")" ]; then
        found=true
        case "$output" in
            *.dmg) echo "DMG ready: $output (SHA-256 $(shasum -a 256 "$output" | cut -d ' ' -f 1))" ;;
            *) echo "App ready: $output ($(lipo -archs "$output/Contents/MacOS/splitty-gui"))" ;;
        esac
    fi
done
if [ "$found" = false ]; then
    echo "the build succeeded but produced nothing new in $bundle_dir" >&2
    exit 1
fi
