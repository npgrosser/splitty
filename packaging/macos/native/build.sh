#!/usr/bin/env bash

# Packs the native macOS desktop app (crates/splitty-gpui) as an ad-hoc signed
# Splitty.app in a disk image:
#
#   Splitty_<version>_aarch64.dmg    (x64 when built on an Intel Mac)
#
# The app is one executable with the server linked in. Its terminals look for
# two things beside it: the `splitty` command, which they get on their PATH,
# and `shell-integration/`. The bundle holds them as
#
#   Splitty.app/Contents/MacOS/splitty-gpui
#   Splitty.app/Contents/MacOS/splitty
#   Splitty.app/Contents/MacOS/shell-integration -> ../Resources/shell-integration
#   Splitty.app/Contents/Resources/shell-integration/
#
# so that only code lies in the code directory and the signature seals the
# scripts as resources.
#
# With --executable the script only packs. Without it, it builds first: the
# frontend, the pinned terminal library unless GHOSTTY_VT_DIR names one, and
# release builds of the app and of the `splitty` command for this Mac. The
# servers for remote hosts are built in when the SPLITTY_REMOTE_PAYLOAD_*
# variables name them (crates/splitty-remote/build.rs); a release sets all
# four. The terminal library is built for the host, so the app is not
# universal.

set -euo pipefail
umask 022

usage() {
    cat >&2 <<'EOF'
usage: packaging/macos/native/build.sh [options]

  --out DIR                 where the disk image goes (default: build/native-macos)
  --executable FILE         a built splitty-gpui; given, nothing is compiled
  --cli FILE                the splitty command to ship (default: target/release/splitty,
                            built here unless --executable is given)
  --shell-integration DIR   default: shell-integration of this checkout
  --version VERSION         default: the workspace version in Cargo.toml
EOF
    exit 2
}

repo_root=$(cd "$(dirname "$0")/../../.." && pwd -P)
target_dir=${CARGO_TARGET_DIR:-$repo_root/target}

out_dir=$repo_root/build/native-macos
executable=
cli=
shell_integration=$repo_root/shell-integration
version=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --out | --executable | --cli | --shell-integration | --version)
            [ "$#" -ge 2 ] || usage
            case "$1" in
                --out) out_dir=$2 ;;
                --executable) executable=$2 ;;
                --cli) cli=$2 ;;
                --shell-integration) shell_integration=$2 ;;
                --version) version=$2 ;;
            esac
            shift
            ;;
        *) usage ;;
    esac
    shift
done

if [ "$(uname -s)" != Darwin ]; then
    echo "this packs the macOS app on a Mac; docs/development.md names the builds for other platforms" >&2
    exit 1
fi
for required_command in codesign hdiutil plutil; do
    if ! command -v "$required_command" >/dev/null 2>&1; then
        echo "missing command: $required_command" >&2
        exit 1
    fi
done

if [ -z "$version" ]; then
    version=$(sed -n '/^\[workspace\.package\]/,/^\[/ s/^version = "\(.*\)"$/\1/p' "$repo_root/Cargo.toml")
fi
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
    echo "not a version: '$version'" >&2
    exit 1
fi
# CFBundleVersion takes numbers only; the short version string shows the rest.
numeric_version=${version%%-*}

if [ -z "$executable" ]; then
    echo "[1/4] Building the frontend, the app and the splitty command"
    (
        cd "$repo_root"
        pnpm --dir frontend install --frozen-lockfile
        pnpm --dir frontend build
        if [ -z "${GHOSTTY_VT_DIR:-}" ]; then
            GHOSTTY_VT_DIR=$(python3 scripts/bootstrap-libghostty-vt.py)
            export GHOSTTY_VT_DIR
        fi
        cargo build --locked --release -p splitty-gpui
        if [ -z "$cli" ]; then
            cargo build --locked --release -p splitty-server
        fi
    )
    executable=$target_dir/release/splitty-gpui
else
    echo "[1/4] Using the built app $executable"
fi
if [ -z "$cli" ]; then
    cli=$target_dir/release/splitty
fi

for file in "$executable" "$cli"; do
    if [ ! -f "$file" ] || [ ! -x "$file" ]; then
        echo "not an executable file: $file" >&2
        exit 1
    fi
done
if [ ! -f "$shell_integration/bash/splitty.bash" ] || [ ! -f "$shell_integration/zsh/.zshenv" ]; then
    echo "not a shell-integration directory: $shell_integration" >&2
    exit 1
fi

case "$(uname -m)" in
    arm64) arch=aarch64 ;;
    x86_64) arch=x64 ;;
    *)
        echo "unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

# The oldest macOS the executable was linked for; 11.0 is the first release
# for Apple silicon.
minimum_system=
if command -v vtool >/dev/null 2>&1; then
    minimum_system=$(vtool -show-build "$executable" 2>/dev/null | awk '$1 == "minos" { print $2; exit }' || true)
fi
if ! [[ "$minimum_system" =~ ^[0-9]+(\.[0-9]+)*$ ]]; then
    minimum_system=11.0
fi

mkdir -p "$out_dir"
out_dir=$(cd "$out_dir" && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/splitty-native-macos.XXXXXX")
trap 'rm -rf "$work"' EXIT

echo "[2/4] Assembling Splitty.app $version ($arch, macOS $minimum_system or later)"

# The disk image's content: the app and a link to drag it onto.
image_root=$work/image
app=$image_root/Splitty.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/shell-integration"
install -m 0755 "$executable" "$app/Contents/MacOS/splitty-gpui"
install -m 0755 "$cli" "$app/Contents/MacOS/splitty"
cp -R "$shell_integration/." "$app/Contents/Resources/shell-integration/"
find "$app/Contents/Resources/shell-integration" -type d -exec chmod 0755 {} +
find "$app/Contents/Resources/shell-integration" -type f -exec chmod 0644 {} +
ln -s ../Resources/shell-integration "$app/Contents/MacOS/shell-integration"
install -m 0644 "$repo_root/icon-macos.icns" "$app/Contents/Resources/icon.icns"
install -m 0644 "$repo_root/LICENSE" "$app/Contents/Resources/LICENSE"

# Its own identifier: the web app (com.splitty.app) stays a separate app to
# the system.
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>Splitty</string>
    <key>CFBundleExecutable</key>
    <string>splitty-gpui</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundleIdentifier</key>
    <string>com.splitty.native</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Splitty</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$numeric_version</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key>
    <string>$minimum_system</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null

echo "[3/4] Signing ad hoc"
# Not notarized: the ad-hoc signature lets the app run on Apple silicon, and
# the README explains the quarantine flag. Inner code first, then the bundle.
codesign --force --sign - "$app/Contents/MacOS/splitty"
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

echo "[4/4] Creating the disk image"
ln -s /Applications "$image_root/Applications"
image=$out_dir/Splitty_${version}_${arch}.dmg
rm -f "$image"
# hdiutil occasionally fails with "Resource busy" on a loaded machine.
attempt=1
until hdiutil create -volname Splitty -srcfolder "$image_root" -fs HFS+ -format UDZO -ov "$image" >/dev/null; do
    if [ "$attempt" -ge 3 ]; then
        echo "hdiutil could not create $image" >&2
        exit 1
    fi
    attempt=$((attempt + 1))
    sleep 5
done

echo ""
echo "DMG ready: $image (SHA-256 $(shasum -a 256 "$image" | cut -d ' ' -f 1))"
