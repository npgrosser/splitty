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

## Frontend transport boundary

The packaged entry completes a native assignment before importing UI modules.
`protocol/connection.ts` then supplies the root `ConnSocket`; host views receive
separate assigned native streams. Browser entries authenticate with their
explicit gateway and pin its role, target and gateway identity before mounting.
Neither shell globals nor URL parameters grant a connection. A failed or
revoked native assignment stays native and never requests a token or ticket.

All backend HTTP-shaped calls use `protocol/http.ts::apiFetch`, which routes
native documents through the enumerated `native_api.ts` operation catalog.
Settings, themes, profiles, templates and remote helpers use this adapter;
unknown native routes, methods, headers or arguments fail before dispatch.
Settings and profile data come from the service rather than the former
loopback origin's browser storage.

The remaining frontend network sites have these purposes:

| Site | Purpose |
| --- | --- |
| `protocol/browser_assignment.ts` | Browser-only authenticated gateway role lookup; skipped after native assignment. |
| `protocol/http.ts` | Browser gateway API calls, session exchanges and single-use WebSocket tickets; native mode rejects authentication/ticket calls and uses native operations for APIs. |
| `protocol/websocket.ts` | Browser root session socket; native startup replaces the default connection before UI imports. |
| `remote/api.ts` | Browser Hub WebSocket; native main documents use their assigned host opener, and native remote documents cannot open another host. |
| Remote helper `deps.fetch` / `fx.fetch` calls | Injected `apiFetch` from the workspace, including host order, preferences, remembered screens and forwarding. |
| `ghostty-web` WASM loader and `FontFace` | Ghostty loads an explicit build-emitted WASM URL; the font is a same-origin packaged resource. Neither uses a backend, CDN or library fallback path. CSS and dynamic module imports are build-produced resources. |

`NativeSocket` handles native heartbeat Ping/Pong controls as well as data
receipts; control replies do not consume application request or delivery slots.
Reconnect discards terminal input and outstanding non-idempotent requests;
uncertain mutations are reported rather than retried. Clipboard and image reads, multi-file
paste/drop batches and delayed Ctrl+V capture one session-client generation.
A disconnect, replacement welcome or disposed view prevents their remaining
bytes and final path insertion from reaching another connection. Confirmed
editor completions likewise cannot reply on a replacement generation.

Dockview popouts use the parent's existing session/connection objects and the
packaged `popout.html`; they start no independent backend connection or native
grant. They share the parent's script trust domain. The parent closes its
Dockview popouts on unload, while native document revocation independently
closes its handles and pending operations. Backend/session IDs are always
resolved inside the owning `ServerConnection`.

## Prerequisites

- Rust (stable)
- Node.js 24 and pnpm
- (Desktop) Tauri v2 prerequisites — see [Tauri docs](https://v2.tauri.app/start/prerequisites/)

Frontend conformance tests import the actual Electron adapters. Install both
`frontend/` and `frontend-electron/` dependencies before running `pnpm check` or
`pnpm test` in `frontend/`. `ELECTRON_SKIP_BINARY_DOWNLOAD=1` can be set for the
Electron dependency install when only types and unit tests are needed. The
Electron build also requires the frontend dependencies to package shared assets.

## Running from source

```bash
# Backend with the embedded frontend on 127.0.0.1:9800, rebuilt on changes
make dev

# Desktop app using packaged assets and native IPC
make dev-tauri
```

For browser development, `make dev` starts an explicit loopback gateway and
prints `SPLITTY_URL`; open that exact URL. It serves the embedded frontend from
the same origin as its API. Rebuild frontend assets before restarting the
backend to pick up UI changes. A separate Vite server and its proxy do not gain
native authority or a browser gateway origin exception.

`make dev-tauri` builds `frontend/dist`, watches it with `vite build --watch`,
and starts `cargo tauri dev`. It loads packaged assets through Tauri's protocol
handler, with no `devUrl` or UI TCP listener. Reload the app after an asset
rebuild. Normal Cargo desktop builds use this packaged path as well; build the
frontend first. On Windows, local terminals still require the app-owned WSL
backend; remote windows use Windows OpenSSH without WSL.

The Linux WebKit smoke (`scripts/testing/tauri-webkit-smoke.py`) supports a
minimal transport fixture and the actual packaged UI. The full UI run covers
CSS/fonts, Ghostty WebAssembly and rendering, heartbeat, binary terminal output,
reload identity, stale-proof denial, parent-mediated popouts, cleanup and no
app/WebKit TCP listeners. A separate daemon and keeper job remain usable after
the app exits. These checks use production capsule, carrier and provider code.

The Windows harnesses build with MSVC and generated ACL/resources. Fresh
WebView2 runs cover Setup/Main/two-Remote isolation, native SSH recovery,
app-owned WSL pipes, binary terminal/file traffic, distribution switching and
app restart. A separate hosted run verifies the full UI with localhost
forwarding disabled, enclosed by positive/negative TCP controls. The fixtures
isolate configuration and distributions; they do not establish arbitrary
host-inventory behavior. Cleanup uses private kernel Jobs assigned before
process start and exact temporary-distribution ownership. Fresh runs pass
listener inventory, independent-process preservation and complete teardown.
Earlier PID-tree cleanup claims remain excluded; see the
[verification status](../openspec/changes/native-client-transports/verification.md).

The opt-in Windows `--full-ui` fixture also passes actual WebView2 production
assets, native settings, binary traffic and bounded popout lifecycle checks.
See the [Windows harness scope](../scripts/testing/tauri-windows-native-smoke/README.md)
for the remaining platform cases and the distinction between app exit and reload.

The actual macOS WKWebView full-UI run passes 37 document generations, packaged
assets, popouts, native traffic and coexistence with a separate daemon. macOS
core acceptance also passed at the recorded pre-review revision. Subsequent review
corrections require fresh targeted checks and CI; the verification status records
which revisions and platform cases have actually passed.

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

## Continuous integration

[CI](../.github/workflows/ci.yml) and [CodeQL](../.github/workflows/codeql.yml)
run on pushes to `main` and pull requests. Changes limited to README files,
project instructions, documentation pages and images, roadmaps or OpenSpec
change drafts skip both workflows. A mixed documentation and code change runs
the checks normally. The ignore lists deliberately keep `docs/protocol.ts`,
the CLI's embedded `agent_guide.md`, and `openspec/specs/` covered; architecture
tests verify that the referenced spec directories exist.

The weekly schedule runs the dependency audit and CodeQL, so new advisories
and analysis queries are checked without rebuilding unchanged code. Both
workflows can also be started manually; a manual CI run includes all tests
and build checks. CodeQL uses the repository workflow for Actions,
JavaScript/TypeScript, Python and Rust; GitHub's default setup must remain
disabled because it overrides custom CodeQL workflows.

Release packages are built on `v*` tags or by manually starting the release
workflow. Ordinary pushes do not publish packages.

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

## Pinned historical rollback smoke

On Linux with Bubblewrap, permitted user/PID/network namespaces and a merged
`/usr` runtime, the daemon lifecycle suite can exercise a verified earlier
server artifact. Build or obtain the intended revision separately, verify its
provenance, and pin its SHA256 before running:

```sh
SPLITTY_TEST_PREVIOUS_BINARY=/absolute/path/to/verified/splitty \
SPLITTY_TEST_PREVIOUS_SHA256='replace-with-verified-64-character-sha256' \
SPLITTY_TEST_REQUIRE_PREVIOUS_BINARY=1 \
cargo test -p splitty-server --test daemon_lifecycle \
  a_kept_shell_survives_a_pinned_previous_binary_rollback_and_return -- --exact --nocapture
```

The test checks current → previous → current using real daemons and keepers,
retaining the same shell PID and private shell state. It runs as PID 1 inside a
private namespace and binds only the three executable artifacts read-only;
its filesystem and loopback listener remain isolated. CLI calls and the outer
supervisor have deadlines. Without an explicitly pinned artifact, the optional
test reports `UNRUN`; strict mode makes that omission a failure. A normal
workspace test pass alone is not historical interoperability evidence.
