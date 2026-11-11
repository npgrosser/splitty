#!/usr/bin/env bash

# Packs the native Linux desktop app (crates/splitty-gpui) as a deb, an rpm
# and a tar.gz for x86_64:
#
#   splitty_<version>_amd64.deb
#   splitty-<version>-1.x86_64.rpm
#   splitty-<version>-linux-x86_64.tar.gz
#
# The app is one executable with the server linked in. Its terminals look for
# two things beside it: the `splitty` command, which they get on their PATH,
# and `shell-integration/`. The packages install all three to /opt/splitty and
# link /usr/bin/splitty-gpui, which the desktop entry starts.
#
# With --executable the script only packs. Without it, it builds first: the
# frontend, the pinned terminal library unless GHOSTTY_VT_DIR names one, and
# release builds of the app and of the `splitty` command. The servers for
# remote hosts are built in when the SPLITTY_REMOTE_PAYLOAD_* variables name
# them (crates/splitty-remote/build.rs); a release sets all four.
#
# Needs dpkg-deb, rpmbuild, objdump and tar.

set -euo pipefail
umask 022

usage() {
    cat >&2 <<'EOF'
usage: packaging/linux/native/build.sh [options]

  --out DIR                 where the packages go (default: build/native-linux)
  --executable FILE         a built splitty-gpui; given, nothing is compiled
  --cli FILE                the splitty command to ship (default: target/release/splitty,
                            built here unless --executable is given)
  --shell-integration DIR   default: shell-integration of this checkout
  --version VERSION         default: the workspace version in Cargo.toml
EOF
    exit 2
}

repo_root=$(cd "$(dirname "$0")/../../.." && pwd -P)
here=$repo_root/packaging/linux/native
target_dir=${CARGO_TARGET_DIR:-$repo_root/target}

out_dir=$repo_root/build/native-linux
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

if [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != x86_64 ]; then
    echo "this packs the Linux x86_64 app; docs/development.md names the builds for other platforms" >&2
    exit 1
fi
for required_command in dpkg-deb rpmbuild objdump tar gzip; do
    if ! command -v "$required_command" >/dev/null 2>&1; then
        echo "missing command: $required_command (Debian/Ubuntu: dpkg-dev, rpm, binutils; Fedora: dpkg, rpm-build)" >&2
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
# A pre-release sorts before its release in both package formats with a tilde
# (0.3.0~rc.1 < 0.3.0); the file names keep the version as it is written.
package_version=${version/-/\~}

if [ -z "$executable" ]; then
    echo "[1/3] Building the frontend, the app and the splitty command"
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
    echo "[1/3] Using the built app $executable"
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

mkdir -p "$out_dir"
out_dir=$(cd "$out_dir" && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/splitty-native-linux.XXXXXX")
trap 'rm -rf "$work"' EXIT

echo "[2/3] Staging Splitty $version"

# What the app needs beside itself, in the layout of the tar.gz.
app=$work/splitty-$version-linux-x86_64
mkdir -p "$app/shell-integration"
install -m 0755 "$executable" "$app/splitty-gpui"
install -m 0755 "$cli" "$app/splitty"
cp -R "$shell_integration/." "$app/shell-integration/"
find "$app/shell-integration" -type d -exec chmod 0755 {} +
find "$app/shell-integration" -type f -exec chmod 0644 {} +

# The installed tree both packages share.
stage=$work/stage
mkdir -p "$stage/opt" "$stage/usr/bin" "$stage/usr/share/applications" \
    "$stage/usr/share/icons/hicolor/512x512/apps"
cp -R "$app" "$stage/opt/splitty"
ln -s ../../opt/splitty/splitty-gpui "$stage/usr/bin/splitty-gpui"
install -m 0644 "$here/splitty.desktop" "$stage/usr/share/applications/splitty.desktop"
install -m 0644 "$repo_root/crates/splitty-tauri/icons/icon.png" \
    "$stage/usr/share/icons/hicolor/512x512/apps/splitty.png"

# The tar.gz runs from wherever it is unpacked; it brings the desktop entry
# and the icon for those who want to install them.
install -m 0644 "$here/splitty.desktop" "$app/splitty.desktop"
install -m 0644 "$repo_root/crates/splitty-tauri/icons/icon.png" "$app/splitty.png"
install -m 0644 "$repo_root/LICENSE" "$app/LICENSE"

echo "[3/3] Packing"

tarball=$out_dir/splitty-$version-linux-x86_64.tar.gz
tar -C "$work" --sort=name --owner=0 --group=0 --numeric-owner \
    -czf "$tarball" "splitty-$version-linux-x86_64"

# --- deb ---
# The newest glibc symbol version the executable asks for; it depends on the
# system the app was built on.
glibc=$(objdump -T "$executable" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
if [ -z "$glibc" ]; then
    echo "cannot read the glibc version $executable needs" >&2
    exit 1
fi
deb_root=$work/deb
mkdir -p "$deb_root/DEBIAN" "$deb_root/usr/share/doc/splitty"
cp -a "$stage/." "$deb_root/"
install -m 0644 "$repo_root/LICENSE" "$deb_root/usr/share/doc/splitty/copyright"
installed_kib=$(du -sk --exclude=DEBIAN "$deb_root" | cut -f 1)
# Depends: what the executable links (readelf -d) and what it loads at run
# time: Wayland, EGL and the Vulkan loader. A Vulkan driver is the system's.
cat > "$deb_root/DEBIAN/control" <<EOF
Package: splitty
Version: $package_version
Architecture: amd64
Maintainer: Splitty contributors <https://github.com/npgrosser/splitty>
Installed-Size: $installed_kib
Depends: libc6 (>= $glibc), libgcc-s1, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-egl1, libegl1, libvulkan1
Recommends: mesa-vulkan-drivers | vulkan-icd, fonts-dejavu-core, fonts-noto-color-emoji
Section: utils
Priority: optional
Homepage: https://github.com/npgrosser/splitty
Description: Multi-session terminal manager
 Splitty is a graphical terminal multiplexer: screens, splits and tabs for
 local terminals and for remote hosts over SSH. This package holds the native
 desktop app and the splitty command its terminals use.
EOF
deb=$out_dir/splitty_${version}_amd64.deb
# xz, which every dpkg reads; Ubuntu's dpkg-deb would write zstd.
dpkg-deb --root-owner-group -Zxz --build "$deb_root" "$deb" >/dev/null

# --- rpm ---
rpm_stage=$work/rpm-stage
mkdir -p "$rpm_stage/usr/share/licenses/splitty"
cp -a "$stage/." "$rpm_stage/"
install -m 0644 "$repo_root/LICENSE" "$rpm_stage/usr/share/licenses/splitty/LICENSE"
rpmbuild -bb --quiet \
    --target x86_64-linux \
    --define "_topdir $work/rpmbuild" \
    --define "_rpmdir $work/rpms" \
    --define "_dbpath $work/rpmdb" \
    --define "_buildhost localhost" \
    --define "splitty_version $package_version" \
    --define "splitty_stage $rpm_stage" \
    "$here/splitty.spec"
built_rpm=$work/rpms/x86_64/splitty-$package_version-1.x86_64.rpm
if [ ! -f "$built_rpm" ]; then
    echo "rpmbuild did not write $built_rpm" >&2
    exit 1
fi
rpm=$out_dir/splitty-$version-1.x86_64.rpm
cp "$built_rpm" "$rpm"

echo ""
for package in "$deb" "$rpm" "$tarball"; do
    echo "Package ready: $package"
done
