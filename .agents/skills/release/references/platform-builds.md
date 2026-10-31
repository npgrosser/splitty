# Local desktop packages

Detect WSL before treating `uname -s = Linux` as a Linux desktop. Use the companion `scripts/downloads.py` probe. Do not change `HOME`, `home` or `CODEX_HOME` to stage builds. Paths with spaces and apostrophes need proper quoting; prefer argument arrays/PowerShell `-File` over interpolated command strings.

## Windows through WSL

Use the existing bridge from the checkout at `release_sha`:

```sh
bash scripts/windows-installer.sh
```

It creates/reuses a Windows-drive checkout, fetches this repository's committed HEAD into it, and runs `packaging/windows/build.ps1` using native Windows Rust/MSVC, Node/pnpm and WSL for backend payloads. Do not pass `--install` or `--skip-tests` by default. A Linux Tauri build inside WSL would produce the wrong desktop package.

Use an existing clean `SPLITTY_WINDOWS_CHECKOUT` when configured. If the default preview checkout contains other work, choose a separate release checkout, e.g. under the Windows user's `Code` folder; never discard the preview's changes. The wrapper requires WSL interop, `powershell.exe`, `wslpath`, native Windows Git, Node/pnpm, Rust/MSVC and the payload prerequisites from `packaging/windows/README.md`. `SPLITTY_WINDOWS_PATH` can add native Windows tools to PATH.

The Windows staging checkout's HEAD must equal `release_sha`. Look in its `target/x86_64-pc-windows-msvc/release/bundle/nsis/` for the installer bearing `VERSION`. Verify it was produced during this build; pass its WSL path to the Downloads helper. The wrapper's current x64 restriction is real: report an unsupported Windows architecture instead of labeling an x64 package as native ARM64.

If the wrapper's interpolated PowerShell command cannot represent a path/distro containing apostrophes, use a clean Windows-drive checkout at `release_sha` and invoke `packaging/windows/build-from-wsl.sh` there; that bridge uses PowerShell `-File` with separate arguments.

## Native Windows

Use a checkout on a Windows drive. Run release versioning through WSL (or Git Bash with Cargo available); `scripts/release.sh` is a Bash script. The local packaging command is:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\packaging\windows\build.ps1
```

Select an existing WSL distribution with `-Distro` if needed. Inspect the same versioned NSIS output as above. Run the helper with `py -3` or an available Python 3; it queries Windows Downloads directly. This build needs WSL for its Linux backend even though the desktop app is native Windows.

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

On Linux, build and check the local frontend with `pnpm install --frozen-lockfile`, `pnpm check` and `pnpm build` in `frontend/` (the macOS script does this itself). CI payloads supply only remote backends; build the desktop package on the current machine.

If payload artifacts are unavailable, build them locally from the release commit with `packaging/windows/build-wsl-payload.sh`: `aarch64`, `darwin-aarch64`, `darwin-x86_64` first, then `x86_64` with the other three payload environment variables set. On macOS, `packaging/macos/build.sh` does this by itself (below). Report which source the payloads came from; never ship missing or older backends.

## macOS

Prerequisites: Xcode command-line tools, Rust (rustup) and Node/pnpm. From the checkout at `release_sha`:

```sh
packaging/macos/build.sh               # native architecture, the default
packaging/macos/build.sh --universal   # only if a universal package is requested
```

It builds the frontend, runs `pnpm check` and the desktop unit tests, and builds the DMG with all four payloads: the archives named by the four `SPLITTY_REMOTE_PAYLOAD_*` variables when all of them are exported (the CI artifacts above), otherwise built from the same commit into `build/remote-payloads/`. It sets `SPLITTY_COMMIT`, `SOURCE_DATE_EPOCH` and `SPLITTY_BUILD_TIME` itself. Do not pass `--skip-tests` by default.

The repository's `tauri.macos.conf.json` supplies ad-hoc signing. The native DMG is under `target/release/bundle/dmg/`; a universal build uses `target/universal-apple-darwin/release/bundle/dmg/`. Inspect the app executable's architecture with `file`/`lipo -info`, verify the bundle version, and copy the matching DMG to the macOS Downloads folder. Do not install into `/Applications` or stop a running app merely to produce a download.

## Native Linux

Prerequisites: Rust, Node/pnpm, WebKitGTK 4.1, GTK/AppIndicator build libraries, `patchelf` and `file`; consult `.github/workflows/release.yml` for the current distribution packages. Build for the current supported machine architecture, with the payload environment above.

From `crates/splitty-tauri`:

```sh
NO_STRIP=true ../../frontend/node_modules/.bin/tauri build --bundles deb,appimage
```

Collect the versioned files under `target/release/bundle/deb/` and `target/release/bundle/appimage/`. Use the actual XDG Downloads folder via the helper. If the user asks for Electron, build the same-commit native `splitty-server` into `target/release/splitty` with the payload environment, then `pnpm install --frozen-lockfile` and `pnpm run dist --publish never` in `frontend-electron/`. Label that AppImage as Electron to distinguish it from Tauri. Do not use `make install` for delivery to Downloads.
