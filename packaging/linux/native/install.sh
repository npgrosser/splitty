#!/bin/sh

# Installs the desktop app from the Linux tar.gz for one user, without a
# package manager:
#
#   <prefix>/lib/splitty/                 splitty-gpui, splitty, shell-integration/
#   <prefix>/bin/splitty-gpui, splitty    links to them
#   <prefix>/share/applications/splitty.desktop
#   <prefix>/share/icons/hicolor/512x512/apps/splitty.png
#
# The prefix is ~/.local unless --prefix names another. Run from the unpacked
# tar.gz, it installs the files beside it. Run without them, for example
# piped from the repository into `sh`, it downloads the tar.gz of the latest
# release (or of --version), checks it against the release's SHA256SUMS and
# installs that. Running it again updates; --uninstall removes the files.

set -eu

repository=https://github.com/npgrosser/splitty
prefix=${HOME:-}/.local
version=
uninstall=false

usage() {
    cat >&2 <<'EOF'
usage: install.sh [--prefix DIR] [--version VERSION] [--uninstall]

  --prefix DIR        where to install (default: ~/.local)
  --version VERSION   the release to download (default: the latest);
                      unused when the app's files are beside this script
  --uninstall         remove what an install put under the prefix
EOF
    exit "$1"
}

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix | --version)
            [ "$#" -ge 2 ] || usage 2
            case "$1" in
                --prefix) prefix=$2 ;;
                --version) version=${2#v} ;;
            esac
            shift 2
            ;;
        --uninstall)
            uninstall=true
            shift
            ;;
        -h | --help) usage 0 ;;
        *) usage 2 ;;
    esac
done

case "$prefix" in
    /?*) ;;
    *) fail "the prefix must be an absolute path: '$prefix'" ;;
esac
prefix=${prefix%/}

app_dir=$prefix/lib/splitty
bin_dir=$prefix/bin
desktop_entry=$prefix/share/applications/splitty.desktop
icon=$prefix/share/icons/hicolor/512x512/apps/splitty.png

refresh_desktop_database() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$prefix/share/applications" >/dev/null 2>&1 || true
    fi
}

if "$uninstall"; then
    rm -rf "$app_dir"
    # Only the links an install made, not another splitty in the same place.
    for name in splitty-gpui splitty; do
        if [ -L "$bin_dir/$name" ] && [ "$(readlink "$bin_dir/$name")" = "../lib/splitty/$name" ]; then
            rm -f "$bin_dir/$name"
        fi
    done
    rm -f "$desktop_entry" "$icon"
    refresh_desktop_database
    echo "Removed Splitty from $prefix. Settings stay in ~/.config/splitty."
    exit 0
fi

# The unpacked tar.gz this script is part of, if it is.
source_dir=
case "$0" in
    */*)
        here=$(cd "$(dirname "$0")" && pwd -P)
        if [ -x "$here/splitty-gpui" ] && [ -x "$here/splitty" ]; then
            source_dir=$here
        fi
        ;;
esac

work=
trap '[ -z "$work" ] || rm -rf "$work"' EXIT

if [ -z "$source_dir" ]; then
    [ "$(uname -s)" = Linux ] || fail "this installs the Linux app; see docs/install.md for $(uname -s)"
    [ "$(uname -m)" = x86_64 ] || fail "the release has no desktop app for $(uname -m), only for x86_64"
    for required_command in curl tar sha256sum; do
        command -v "$required_command" >/dev/null 2>&1 || fail "missing command: $required_command"
    done
    if [ -z "$version" ]; then
        # The latest release redirects to its tag.
        latest=$(curl --fail --silent --show-error --location --head --output /dev/null \
            --write-out '%{url_effective}' "$repository/releases/latest")
        version=${latest##*/v}
    fi
    case "$version" in
        '' | *[!0-9A-Za-z.-]*) fail "cannot tell the version of the latest release; name one with --version" ;;
    esac
    work=$(mktemp -d "${TMPDIR:-/tmp}/splitty-install.XXXXXX")
    archive=splitty-$version-linux-x86_64.tar.gz
    echo "Downloading Splitty $version"
    curl --fail --location --progress-bar --output "$work/$archive" \
        "$repository/releases/download/v$version/$archive" ||
        fail "cannot download $archive of release v$version"
    curl --fail --silent --show-error --location --output "$work/SHA256SUMS" \
        "$repository/releases/download/v$version/SHA256SUMS" ||
        fail "cannot download SHA256SUMS of release v$version"
    expected=$(awk -v file="$archive" '$2 == file || $2 == "*" file { print $1 }' "$work/SHA256SUMS")
    actual=$(sha256sum "$work/$archive" | cut -d ' ' -f 1)
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        fail "$archive does not match the checksum in the release's SHA256SUMS"
    fi
    tar -xzf "$work/$archive" -C "$work"
    source_dir=$work/splitty-$version-linux-x86_64
fi

for file in splitty-gpui splitty shell-integration/bash/splitty.bash splitty.desktop splitty.png; do
    [ -f "$source_dir/$file" ] || fail "not an unpacked Splitty tar.gz: $source_dir has no $file"
done

mkdir -p "$prefix/lib" "$bin_dir" "$(dirname "$desktop_entry")" "$(dirname "$icon")"

# Assembled beside its place and moved there, so a running app keeps the
# directory it started from until then.
staged=$app_dir.new.$$
replaced=$app_dir.old.$$
rm -rf "$staged"
mkdir "$staged"
cp "$source_dir/splitty-gpui" "$source_dir/splitty" "$staged/"
cp -R "$source_dir/shell-integration" "$staged/shell-integration"
if [ -f "$source_dir/LICENSE" ]; then
    cp "$source_dir/LICENSE" "$staged/LICENSE"
fi
chmod 0755 "$staged/splitty-gpui" "$staged/splitty"
if [ -e "$app_dir" ]; then
    mv "$app_dir" "$replaced"
fi
mv "$staged" "$app_dir"
rm -rf "$replaced"

ln -sf ../lib/splitty/splitty-gpui "$bin_dir/splitty-gpui"
ln -sf ../lib/splitty/splitty "$bin_dir/splitty"

# By path, so the entry works whatever the session's PATH and icon theme are.
sed -e "s|^Exec=.*|Exec=$bin_dir/splitty-gpui|" -e "s|^Icon=.*|Icon=$icon|" \
    "$source_dir/splitty.desktop" > "$desktop_entry"
cp "$source_dir/splitty.png" "$icon"
chmod 0644 "$desktop_entry" "$icon"
refresh_desktop_database

echo "Installed Splitty to $prefix: start it from the application menu or as $bin_dir/splitty-gpui"
if ! problem=$("$app_dir/splitty-gpui" --help 2>&1 >/dev/null); then
    printf '%s\n' "$problem" | head -n 1 >&2
    cat >&2 <<'EOF'
The app does not start yet; a library it needs is probably missing. It links
libxcb, libxkbcommon and libxkbcommon-x11 and loads Wayland, EGL and the
Vulkan loader, and it needs glibc 2.35 or later and a Vulkan driver.
EOF
fi
case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) echo "$bin_dir is not on your PATH; add it to run splitty-gpui and splitty by name." ;;
esac
