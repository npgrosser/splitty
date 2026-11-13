# Local desktop packages

The local build is the native app (`crates/splitty-gpui`), the only package a release publishes for each system. The legacy Tauri web app is not released and is built locally only when requested; `docs/development.md` names its build.

Detect WSL before treating `uname -s = Linux` as a Linux desktop. Use the companion `scripts/downloads.py` probe. Do not change `HOME`, `home` or `CODEX_HOME` to stage builds. Paths with spaces and apostrophes need proper quoting; prefer argument arrays/PowerShell `-File` over interpolated command strings.

## Windows through WSL

Use the existing bridge from the checkout at `release_sha`:

```sh
bash scripts/windows-installer.sh
```

It creates/reuses a Windows-drive checkout, fetches this repository's committed HEAD into it, and runs `packaging/windows/build.ps1` there (native app by default) using native Windows Rust/MSVC, Node/pnpm, NSIS (`makensis.exe`, also found in the Tauri CLI's cache) and WSL for the backend payload. It prints `Setup: <windows path>` for the built installer. Do not pass `--install`, `--skip-tests` or `--web` (the legacy Tauri app) by default. A Linux build inside WSL would produce the wrong desktop package.

The native executable links the pinned terminal library. `SPLITTY_GHOSTTY_VT_DIR` names the Windows path of a built prefix; without it the build runs `scripts/bootstrap-libghostty-vt.py` with Windows Python, which downloads and builds it once into the user cache.

Use an existing clean `SPLITTY_WINDOWS_CHECKOUT` when configured. If the default preview checkout contains other work, choose a separate release checkout, e.g. under the Windows user's `Code` folder; never discard the preview's changes. The wrapper requires WSL interop, `powershell.exe`, `wslpath`, native Windows Git, Node/pnpm, Rust/MSVC and the payload prerequisites from `packaging/windows/README.md`. `SPLITTY_WINDOWS_PATH` can add native Windows tools to PATH.

The Windows staging checkout's HEAD must equal `release_sha`. The native installer is `target/x86_64-pc-windows-msvc/release/bundle/native/Splitty_<VERSION>_x64-setup.exe` (the path the wrapper printed); the legacy one, when requested, is under `bundle/nsis/` and is named `Splitty Web_<VERSION>_x64-setup.exe` (the release calls it `Splitty-Web_<VERSION>_x64-setup.exe`). Verify the file was produced during this build; pass its WSL path to the Downloads helper. The wrapper's current x64 restriction is real: report an unsupported Windows architecture instead of labeling an x64 package as native ARM64.

If the wrapper's interpolated PowerShell command cannot represent a path/distro containing apostrophes, use a clean Windows-drive checkout at `release_sha` and invoke `packaging/windows/build-from-wsl.sh` there; that bridge uses PowerShell `-File` with separate arguments.

## Native Windows

Use a checkout on a Windows drive. Run release versioning through WSL (or Git Bash with Cargo available); `scripts/release.sh` is a Bash script. The local packaging command is:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\packaging\windows\build.ps1
```

`-App native` is the default; `-App web` builds the legacy Tauri app. Select an existing WSL distribution with `-Distro` if needed. Inspect the same versioned output as above. Run the helper with `py -3` or an available Python 3; it queries Windows Downloads directly. This build needs WSL for its Linux backend even though the desktop app is native Windows.

## Shared payloads for native macOS and Linux

The desktop app must carry backend payloads for supported remote hosts. The workflow builds these before its desktop jobs. Prefer the artifacts from the **same release run and commit**, never the latest unrelated run/release. Download each artifact into a fresh release-specific payload directory once the payload job has uploaded it:

```sh
gh run download "$run_id" --name wsl-payload --dir "$payload_dir"
gh run download "$run_id" --name remote-payload-aarch64 --dir "$payload_dir"
gh run download "$run_id" --name remote-payload-darwin --dir "$payload_dir"
```

Verify all four `.sha256` files inside that directory using `sha256sum -c` on Linux or `shasum -a 256 -c` on macOS. Set absolute archive paths for the local desktop build:

```sh
export SPLITTY_REMOTE_PAYLOAD_X86_64="$payload_dir/splitty-wsl-x86_64.tar.gz"
export SPLITTY_REMOTE_PAYLOAD_AARCH64="$payload_dir/splitty-wsl-aarch64.tar.gz"
export SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64="$payload_dir/splitty-darwin-aarch64.tar.gz"
export SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64="$payload_dir/splitty-darwin-x86_64.tar.gz"
export SPLITTY_COMMIT="$release_sha"
export SOURCE_DATE_EPOCH=0
export SPLITTY_BUILD_TIME=1970-01-01T00:00:00Z
```

The native Linux and macOS executables build these archives in when the variables are set, and ship the matching static server as their `splitty` command. Unpack the one for the current machine (`splitty-wsl-x86_64.tar.gz` on Linux, `splitty-darwin-aarch64.tar.gz` on an Apple silicon Mac) into a fresh directory; it holds `splitty` and `shell-integration/`. The CI payloads provide the server binaries; build the desktop package on the current machine.

If payload artifacts are unavailable, build them locally from the release commit with `packaging/windows/build-wsl-payload.sh`: `aarch64`, `darwin-aarch64`, `darwin-x86_64` first, then `x86_64` with the other three payload environment variables set. Report which source the payloads came from; never ship missing or older backends.

## macOS

Prerequisites: Xcode command-line tools, Rust (rustup), Python 3 and Node/pnpm. On an Apple silicon Mac, from the checkout at `release_sha` with the payload environment above:

```sh
packaging/macos/native/build.sh \
  --cli "$payload_dir/darwin-aarch64/splitty" \
  --shell-integration "$payload_dir/darwin-aarch64/shell-integration"
```

It builds the frontend, the pinned terminal library (unless `GHOSTTY_VT_DIR` names a built prefix) and a release build of `splitty-gpui`, assembles the ad-hoc signed `Splitty.app`, verifies its signature and writes `build/native-macos/Splitty_<VERSION>_aarch64.dmg` (`--out DIR` chooses another directory). Without `--cli` it builds and ships this Mac's own `splitty` instead of the release's static server; say so if that was used. The app is built for the host's architecture: an Intel Mac produces `Splitty_<VERSION>_x64.dmg`, which the release does not publish; report that instead of labeling it as the release package.

Inspect the app executable's architecture with `file`/`lipo -info`, verify the bundle version, and copy the DMG to the macOS Downloads folder. Do not install into `/Applications` or stop a running app merely to produce a download.

The legacy web app (only when requested) is `packaging/macos/build.sh` (`--universal` for a universal DMG); it builds missing payloads itself and writes under `target/release/bundle/dmg/` or `target/universal-apple-darwin/release/bundle/dmg/`.

## Native Linux

Prerequisites: Rust, Python 3, Node.js 24 and pnpm, `dpkg-deb`, `rpmbuild` (Debian/Ubuntu: `rpm`; Fedora: `rpm-build`, `dpkg`) and the development packages `docs/development.md` lists under "The native app". Build for the supported x86_64 architecture, from the checkout at `release_sha` with the payload environment above:

```sh
packaging/linux/native/build.sh \
  --cli "$payload_dir/x86_64/splitty" \
  --shell-integration "$payload_dir/x86_64/shell-integration"
```

It builds the frontend, the pinned terminal library (unless `GHOSTTY_VT_DIR` names a built prefix) and a release build of `splitty-gpui`, and writes `splitty_<VERSION>_amd64.deb`, `splitty-<VERSION>-1.x86_64.rpm` and `splitty-<VERSION>-linux-x86_64.tar.gz` into `build/native-linux/` (`--out DIR` chooses another directory). Without `--cli` it builds and ships this machine's own `splitty` instead of the release's static server; say so if that was used. The deb requires the glibc of the build machine or newer; the release builds on Ubuntu 22.04.

Check the packages (`dpkg-deb --info`, `rpm -qip`, and `./splitty-gpui --help` from the unpacked tar.gz) and copy all three to the actual XDG Downloads folder with the helper. Do not install them or use `make install-*` for delivery to Downloads.
