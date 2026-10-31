#!/usr/bin/env sh

set -eu

if [ -f "$HOME/.cargo/env" ]; then
    # wsl.exe --exec does not start a login shell, so rustup's PATH setup may
    # otherwise be missing even though Rust is installed for the WSL user.
    . "$HOME/.cargo/env"
fi

if [ "$#" -lt 2 ] || [ "$#" -gt 4 ]; then
    echo "usage: $0 <repo-root> <output-directory> [commit] [x86_64|aarch64|darwin-x86_64|darwin-aarch64]" >&2
    exit 2
fi

repo_root=$1
output_dir=$2
commit=${3:-local}
arch=${4:-x86_64}
# Linux (WSL, CI) or a Mac (packaging/macos/build.sh).
build_kernel=$(uname -s)
# The x86_64 archive is also the Windows app's WSL backend; the name stays.
archive_name=splitty-wsl-${arch}.tar.gz
case "$arch" in
    x86_64)
        target=x86_64-unknown-linux-musl
        if [ "$build_kernel" = Darwin ]; then
            # A Mac has no musl-gcc; rust-lld links it as it does aarch64.
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
        fi
        ;;
    aarch64)
        # No C is compiled for the target, so Rust's self-contained musl
        # objects and rust-lld link it without a C cross toolchain.
        target=aarch64-unknown-linux-musl
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
        ;;
    darwin-x86_64 | darwin-aarch64)
        # macOS hosts: on Linux cross-linked by zig, without an Apple SDK, on
        # a Mac by Xcode's linker; the server links only the system's own
        # libraries.
        target=${arch#darwin-}-apple-darwin
        archive_name=splitty-darwin-${arch#darwin-}.tar.gz
        ;;
    *)
        echo "unsupported architecture: $arch (x86_64, aarch64, darwin-x86_64 or darwin-aarch64)" >&2
        exit 2
        ;;
esac
checksum_name=${archive_name}.sha256
build_time=1970-01-01T00:00:00Z

case "$repo_root" in
    /*) ;;
    *)
        echo "repo root must be an absolute path (a Linux path under WSL): $repo_root" >&2
        exit 2
        ;;
esac

if [ "$build_kernel" = Darwin ]; then
    sha256="shasum -a 256"
    required_commands="cargo rustup file tar gzip shasum mktemp grep find"
else
    sha256=sha256sum
    required_commands="cargo rustup file tar gzip sha256sum mktemp grep find"
    if [ "$arch" = x86_64 ]; then
        required_commands="$required_commands musl-gcc"
    fi
fi
for required_command in $required_commands; do
    if ! command -v "$required_command" >/dev/null 2>&1; then
        echo "missing build command: $required_command" >&2
        if [ "$required_command" = musl-gcc ] || [ "$required_command" = file ]; then
            echo "Ubuntu/Debian: sudo apt update && sudo apt install musl-tools file" >&2
        fi
        exit 1
    fi
done

if [ ! -f "$repo_root/Cargo.toml" ] || [ ! -d "$repo_root/shell-integration" ]; then
    echo "not a Splitty repository: $repo_root" >&2
    exit 2
fi

build=build
case "$build_kernel-$target" in
    Darwin-*) ;;
    *-apple-darwin)
        toolchain_path=$(sh "$repo_root/packaging/zig-toolchain.sh")
        PATH=$toolchain_path:$PATH
        export PATH
        build=zigbuild
        ;;
esac

# Compiling into the WSL filesystem is substantially faster than writing Cargo's
# many small files through /mnt/c. CI and native Linux keep the workspace target.
if [ -n "${SPLITTY_WSL_TARGET_DIR:-}" ]; then
    cargo_target_dir=$SPLITTY_WSL_TARGET_DIR
elif [ -n "${WSL_INTEROP:-}" ] || [ -n "${WSL_DISTRO_NAME:-}" ]; then
    cargo_target_dir=$HOME/.cache/splitty/windows-payload-target
else
    cargo_target_dir=$repo_root/target
fi

mkdir -p "$cargo_target_dir" "$output_dir"
rustup target add "$target"

echo "Building $arch server (target cache: $cargo_target_dir)"
SPLITTY_COMMIT=$commit \
SPLITTY_BUILD_TIME=$build_time \
CARGO_TARGET_DIR=$cargo_target_dir \
    cargo "$build" \
        --manifest-path "$repo_root/Cargo.toml" \
        --locked \
        --release \
        --package splitty-server \
        --target "$target"

backend=$cargo_target_dir/$target/release/splitty
if [ ! -x "$backend" ]; then
    echo "backend was not produced at $backend" >&2
    exit 1
fi

file "$backend"
case "$target" in
    *-apple-darwin)
        if ! file "$backend" | grep -q "Mach-O 64-bit"; then
            echo "backend is not a macOS executable" >&2
            exit 1
        fi
        ;;
    *)
        if ! file "$backend" | grep -Eq "statically linked|static-pie linked"; then
            echo "backend is not statically linked" >&2
            exit 1
        fi
        ;;
esac

staging=$(mktemp -d "${TMPDIR:-/tmp}/splitty-wsl-payload.XXXXXX")
archive_tmp=$output_dir/.${archive_name}.$$
checksum_tmp=$output_dir/.${checksum_name}.$$

cleanup() {
    rm -rf "$staging"
    rm -f "$archive_tmp" "$checksum_tmp"
}
trap cleanup EXIT HUP INT TERM

payload_dir=$staging/payload
mkdir -p "$payload_dir/shell-integration"
cp "$backend" "$payload_dir/splitty"
cp -R "$repo_root/shell-integration/." "$payload_dir/shell-integration/"
chmod 755 "$payload_dir/splitty"
if [ "$build_kernel" = Darwin ]; then
    # BSD touch and tar have no --date, --sort or --mtime: the times are set
    # in UTC and the entries are listed in name order, without macOS metadata.
    stamp=$(date -u -r "${SOURCE_DATE_EPOCH:-0}" +%Y%m%d%H%M.%S)
    find "$payload_dir" -exec env TZ=UTC0 touch -t "$stamp" {} +
    (cd "$payload_dir" && find . | LC_ALL=C sort) |
        COPYFILE_DISABLE=1 tar \
            --format ustar \
            --no-mac-metadata \
            --no-xattrs \
            --uid 0 \
            --gid 0 \
            --numeric-owner \
            -n \
            -C "$payload_dir" \
            -T - \
            -cf "$staging/payload.tar"
else
    find "$payload_dir" -exec touch --date="@${SOURCE_DATE_EPOCH:-0}" {} +

    tar \
        --sort=name \
        --mtime="@${SOURCE_DATE_EPOCH:-0}" \
        --owner=0 \
        --group=0 \
        --numeric-owner \
        -C "$payload_dir" \
        -cf "$staging/payload.tar" \
        .
fi
gzip -n -c "$staging/payload.tar" > "$archive_tmp"

checksum_line=$($sha256 "$archive_tmp")
checksum=${checksum_line%% *}
printf '%s  %s\n' "$checksum" "$archive_name" > "$checksum_tmp"

mv -f "$archive_tmp" "$output_dir/$archive_name"
mv -f "$checksum_tmp" "$output_dir/$checksum_name"

echo "Created $output_dir/$archive_name"
echo "SHA-256: $checksum"
