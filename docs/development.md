# Development

How to build, run and release Splitty from a checkout. How it is built inside
is in [`architecture.md`](architecture.md) and the module map in
[`architecture/`](../architecture/README.md); what the system guarantees is in
[`openspec/specs/`](../openspec/specs/). The checks to run before a merge are
listed in [`CLAUDE.md`](../CLAUDE.md#checks).

## Tech stack

| Layer | Technology |
|-------|-----------|
| Frontend | Svelte 5, TypeScript, Vite, Tailwind CSS |
| Terminal | xterm.js with WebGL, ligatures, search, image support; ghostty-web |
| Layout | dockview-core |
| Desktop | Tauri v2 (OS WebView); Electron (Linux AppImage) |
| Backend | Rust (Tokio, Axum) |
| PTY | portable-pty |
| Config | TOML (`~/.config/splitty/config.toml`) |
| Templates | YAML (`~/.config/splitty/templates/`) |

## Prerequisites

- Rust (stable)
- Node.js 24 and pnpm
- (Desktop) Tauri v2 prerequisites — see [Tauri docs](https://v2.tauri.app/start/prerequisites/)

## Running from source

```bash
# Backend with the embedded frontend on 127.0.0.1:9800, rebuilt on changes
make dev

# Frontend with hot reload on :1420, proxying the API to :9800
cd frontend && pnpm install && pnpm dev

# Desktop app in development mode
make dev-tauri
```

`make dev` prints `SPLITTY_URL`; open it, or `http://localhost:1420/?bootstrap=<SPLITTY_BOOTSTRAP>` for hot reload.

## Production builds

```bash
make build/splitty        # server and CLI binary (embeds the frontend)
make build/splitty-gui    # Tauri desktop binary (Linux)
make build/Splitty.app    # macOS app bundle
make dmg                  # macOS disk image
make electron             # Electron AppImage (Linux)
```

## Local macOS app

The macOS app carries the servers it installs on remote hosts (Linux and macOS,
x86_64 and aarch64). One script builds them from the checkout, as the release
does, and then the ad-hoc signed app; `make build/Splitty.app` and `make dmg`
run it. It needs Xcode's command line tools, Rust (rustup), Node.js and pnpm:

```bash
packaging/macos/build.sh               # → target/release/bundle/dmg/
packaging/macos/build.sh --universal   # Apple silicon and Intel → target/universal-apple-darwin/release/bundle/dmg/
```

`--skip-tests` skips the frontend check and the desktop unit tests, and
`--bundles app` builds only `Splitty.app`. The Linux servers are linked with
`rust-lld`, the macOS servers by Xcode, into `build/remote-payloads/`. When all
four `SPLITTY_REMOTE_PAYLOAD_*` variables are set (a release run's payload
artifacts, for example), the script takes those archives instead.

## Local Windows installer

The Windows package combines a static Linux backend built in WSL with the
native Tauri/WebView2 application built by MSVC. From a checkout on a Windows
drive, one PowerShell command builds both and creates the per-user NSIS
installer:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\packaging\windows\build.ps1 -Distro Ubuntu
```

It can also be started from a WSL shell when the checkout is below `/mnt/c`:

```bash
./packaging/windows/build-from-wsl.sh
```

See [the Windows packaging guide](../packaging/windows/README.md) for prerequisites,
cache behavior, alternative distributions, and fast iteration options.

## Installation from source

```bash
# Desktop app (Tauri) — includes both GUI and server binary
make install-tauri
# macOS: → /Applications/Splitty.app
# Linux: → ~/.local/bin/splitty-gui + splitty + desktop entry

# Electron AppImage (Linux)
make install-electron     # → ~/.local/bin/Splitty-Electron.AppImage

# Server only (no GUI)
make install-no-gui       # → ~/.local/bin/splitty

# Uninstall
make uninstall-tauri      # remove app, binary, desktop entry, icon
make uninstall-electron
make uninstall-no-gui     # remove server binary
make uninstall-config     # remove ~/.config/splitty (separate step)
```

## Releases

The shared [release skill](../.agents/skills/release/SKILL.md) is available
as `$release` in Codex and `/release` in Claude Code. It creates
and pushes a version tag, checks the GitHub Actions run, builds the current
host's desktop package, and copies it with a SHA-256 checksum to Downloads.
WSL builds the Windows installer and uses the Windows host's Downloads folder.
The workflow publishes the GitHub release; the skill does not install the app.

Every release has a version of its own: hosts and the Windows WSL backend
install a server per version, so a build under an old version would leave their
older server in place. One command sets the version in `Cargo.toml`,
`Cargo.lock`, the Tauri and the Electron configuration, commits and tags:

```bash
scripts/release.sh 0.1.2
git push --atomic origin HEAD v0.1.2
```

The pushed tag starts the release workflow. It refuses a tag that does not match
the version files, builds the Windows installer, the Linux packages and the
macOS `.dmg`, and publishes the GitHub release with them once all of them have
built; a tag with a suffix (`v0.2.0-rc.1`) becomes a prerelease. The Windows installer is a per-user setup: run a newer one over an
installed Splitty to update it, and uninstall it from Windows' installed apps.
The **Windows Tauri (WSL backend)** workflow, started by hand, builds only the
installer for any commit, as a workflow artifact.

After a successful release run, **Cleanup release artifacts** deletes that
run's Actions artifacts; the files attached to the release remain available.
New builds retain artifacts for at most three days, including failed releases
and standalone Windows builds. Reruns of older tags use the retention settings
from that tag, but successful release reruns are also cleaned up.

On a Windows PC with the Windows Rust toolchain, Node and pnpm, the installer
also builds locally in a few minutes, from WSL:

```bash
scripts/windows-installer.sh --install
```

It brings a checkout on a Windows drive (`SPLITTY_WINDOWS_CHECKOUT`, by default
`%USERPROFILE%\Code\splitty-windows-preview`, cloned when missing) to the
committed `HEAD`, runs `packaging/windows/build.ps1` there and, with `--install`,
starts the setup. `SPLITTY_WINDOWS_PATH` adds Windows directories to the build's
`PATH`, for a pnpm that is not on it; `--skip-tests` skips the type check and the
Windows unit tests. Uncommitted changes are not built, and the version is the one
the sources carry.
