# Development

How to build, run and release Splitty from a checkout. How it is built inside
is in [`architecture.md`](architecture.md) and the module map in
[`architecture/`](../architecture/README.md); what the system guarantees is in
[`openspec/specs/`](../openspec/specs/). The checks to run before a merge are
listed in [`CLAUDE.md`](../CLAUDE.md#checks).

## Tech stack

| Layer | Technology |
|-------|-----------|
| Frontend (web app, browser) | Svelte 5, TypeScript, Vite, Tailwind CSS |
| Terminal (web app, browser) | xterm.js with WebGL, ligatures, search, image support; ghostty-web |
| Layout | dockview-core |
| Desktop | Native app: GPUI with `libghostty-vt` (`crates/splitty-gpui`; Windows, macOS, Linux). Legacy web app: Tauri v2 (Windows, macOS; Linux source build) and Electron (Linux: AppImage, deb, rpm) |
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
- (native app) Python 3 and a C/C++ toolchain for the terminal library; on
  Linux the development packages named under [The native app](#the-native-app)
- (web app, Tauri builds) Tauri v2 prerequisites — see [Tauri docs](https://v2.tauri.app/start/prerequisites/)

Frontend conformance tests import the actual Electron adapters. Install both
`frontend/` and `frontend-electron/` dependencies before running `pnpm check` or
`pnpm test` in `frontend/`. `ELECTRON_SKIP_BINARY_DOWNLOAD=1` can be set for the
Electron dependency install when only types and unit tests are needed. The
Electron build also requires the frontend dependencies to package shared assets.

## Running from source

The native app (`crates/splitty-gpui`) is the desktop app releases install;
[The native app](#the-native-app) below describes its build and packages, the
[GPUI runbook](gpui-spike.md) its smoke modes and acceptance records, and the
[native UI handoff](native-ui-handoff.md) its state and open work. The web
app's shells (Tauri, Electron) are legacy: they keep building and are released
with "Web" in their names.

The workspace applies an explicit path patch to the pinned Linux platform
dependency in `vendor/gpui-pre-linux`. Its [patch notes](../vendor/gpui-pre-linux/SPLITTY-PATCH.md)
record the published source, Apache-2.0 license and finite event-dispatch patch. Keep that directory when
building from a checkout; no edits to the Cargo registry cache are needed.
The patch preserves foreground progress during sustained X11 events while the
application's bounded command/update queues and captured input routes remain
unchanged. A dependency upgrade must pass the platform regressions and the
native multi-source input/window check before this patch can be removed.

### Native terminal library for GPUI

The native Windows app is built with Windows Rust/MSVC; its backend is the
static Linux server, which runs in a WSL distribution. Started without
arguments the app picks the distribution and installs the backend that ships
beside it; the [Windows preview runbook](gpui-spike.md#native-windows-preview)
describes the explicit `--wsl-*` arguments for isolated previews and tests.
The frontend assets must be built before compiling the Linux backend, which
embeds them.

The GPUI app uses statically linked `libghostty-vt` 1.0.0. It needs a matching
native library for the machine running the app: Linux and macOS build it for
their own host, and the Windows GPUI client needs a Windows library even when
its service runs in WSL. Python 3 and a native C/C++ toolchain are required.
On macOS, install the Xcode command-line tools; the build uses Zig 0.17.0.

Run the explicit bootstrap from the repository root. It downloads the pinned
Ghostty source and dependency archives and the matching Zig 0.17.0 toolchain,
checks their SHA-256 hashes, and builds with the supplied offline dependencies
and a baseline CPU target. It prints an absolute install prefix. Re-running it
reuses a verified build in the user cache. Cargo itself never downloads or
builds Ghostty; it requires `GHOSTTY_VT_DIR` to point at that prefix.
Pass `--check` to compile and run a small Rust static-link check after setup.

On Linux or macOS:

```sh
prefix=$(python3 scripts/bootstrap-libghostty-vt.py)
GHOSTTY_VT_DIR="$prefix" cargo run --release -p splitty-gpui
```

On Windows PowerShell:

```powershell
$prefix = python scripts/bootstrap-libghostty-vt.py
$env:GHOSTTY_VT_DIR = $prefix
cargo build --locked --release -p splitty-gpui
```

The script also accepts `--prefix` with an absolute path. An existing prefix
that is not a matching bootstrap build is rejected; choose a new prefix.
The GPUI build verifies the installed 1.0.0 headers and static archive before
linking. On Linux and macOS the app links the server, which embeds
`frontend/dist`: build the frontend first.

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

## The native app

`crates/splitty-gpui` builds one executable, `splitty-gpui`. On Linux and macOS
it links the server and runs it in its own process (the embedded service); on
Windows it starts the static Linux server in WSL. Its local terminals look for
two things beside the executable, where it was started from: the `splitty`
command, which they get on their `PATH`, and the `shell-integration/` folder.
Every package ships both. The servers for remote hosts are built into the
Linux and macOS executables when the build sets the four
`SPLITTY_REMOTE_PAYLOAD_*` variables (`crates/splitty-remote/build.rs`), as a
release does; the Windows executable is built without them, because it reaches
hosts through its WSL backend.

Every build needs the pinned terminal library
([above](#native-terminal-library-for-gpui)); the Linux and macOS builds need
`frontend/dist`.

### Linux

Build packages on Ubuntu or Debian (x86_64):

```bash
sudo apt install build-essential pkg-config rpm libxkbcommon-x11-dev \
  libwayland-dev libx11-dev libxcb1-dev libfontconfig1-dev libfreetype6-dev \
  libvulkan-dev
packaging/linux/native/build.sh        # or: make native-package
```

It builds the frontend, the terminal library (unless `GHOSTTY_VT_DIR` names
one), the app and the `splitty` command, and writes into `build/native-linux/`:

| File | Content |
|---|---|
| `splitty_<version>_amd64.deb`, `splitty-<version>-1.x86_64.rpm` | Package `splitty`: `/opt/splitty/` (`splitty-gpui`, `splitty`, `shell-integration/`), the link `/usr/bin/splitty-gpui`, `splitty.desktop` and the icon |
| `splitty-<version>-linux-x86_64.tar.gz` | The same files in one folder, with the desktop entry, the icon and the license |

`--executable FILE` packs a built `splitty-gpui` without compiling anything;
`--cli FILE` ships another `splitty` command (the release ships the static
one), and `--out DIR` chooses the directory. The deb names its libraries by
package (what the executable links, plus Wayland, EGL and the Vulkan loader,
which it loads at run time) and the glibc version of the build system; the rpm
names them by library, which Fedora and openSUSE share. The release builds on
Ubuntu 22.04, so its packages need glibc 2.35 or later. A pre-release version
is written with a tilde inside the packages (`0.3.0~rc.1`), so the release
that follows counts as newer. The legacy Electron packages (package
`splitty-electron`, `/opt/Splitty`, `splitty-web.desktop`) share no file with
these and install beside them.

### macOS

On a Mac with Xcode's command line tools:

```bash
packaging/macos/native/build.sh        # or: make native-package
```

It builds like the Linux script and writes
`build/native-macos/Splitty_<version>_aarch64.dmg` (`x64` on an Intel Mac; the
terminal library is built for the host, so the app is not universal; the
release builds one on a runner of each architecture). `Splitty.app` holds `splitty-gpui`
and `splitty` in `Contents/MacOS`, `shell-integration/` in `Contents/Resources`
with a link from `Contents/MacOS`, and is ad-hoc signed. Its bundle identifier
is `com.splitty.native`; the web app keeps `com.splitty.app`. Both bundles are
named `Splitty.app`. The options are those of the Linux script.

### Windows

The installer combines the native executable with the static Linux backend
built in WSL. From a checkout on a Windows drive:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\packaging\windows\build.ps1 -Distro Ubuntu
```

`-App native` is the default and writes
`target\x86_64-pc-windows-msvc\release\bundle\native\Splitty_<version>_x64-setup.exe`;
`-App web` builds the legacy Tauri app instead. The last step is
`packaging/windows/native/bundle.ps1`, which packs a built `splitty-gpui.exe`
and the backend archive with NSIS (`makensis.exe` on the `PATH`, in the NSIS
install directory or in the Tauri CLI's cache). The installer keeps the
product name, install directory and uninstall entry of the web app of earlier
releases and replaces it in place. See
[the Windows packaging guide](../packaging/windows/README.md) for prerequisites,
cache behavior, alternative distributions and fast iteration options, and
[Releases](#releases) for the build from WSL.

## The web app's packages

The legacy shells are not released (v0.3.0 was the last release with their
packages); CI keeps them building, and they build locally:

```bash
make build/splitty        # server and CLI binary (embeds the frontend)
make electron             # Linux: Electron AppImage, deb, rpm (needs rpmbuild and libcrypt.so.1; Fedora: rpm-build, libxcrypt-compat)
make build/splitty-gui    # Tauri desktop binary (Linux source build)
make build/Splitty.app    # macOS app bundle (Tauri)
make dmg                  # macOS disk image (Tauri)
```

### macOS (Tauri)

The macOS web app carries the servers it installs on remote hosts (Linux and macOS,
x86_64 and aarch64). One script builds them from the checkout and then the ad-hoc signed app; `make build/Splitty.app` and `make dmg`
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

### Windows (Tauri)

`packaging\windows\build.ps1 -App web` builds the Tauri/WebView2 application
with MSVC and the same WSL-built backend, and creates its per-user NSIS
installer under `target\x86_64-pc-windows-msvc\release\bundle\nsis\`. It
installs as "Splitty Web" (`tauri.windows.conf.json`) beside the native app,
with the application identifier it always had.

Either app also builds from a WSL shell when the checkout is below `/mnt/c`;
the script passes its arguments on to `build.ps1`:

```bash
./packaging/windows/build-from-wsl.sh            # native app
./packaging/windows/build-from-wsl.sh -App web   # legacy Tauri app
```

## Installation from source

```bash
# Web app on Linux: Electron AppImage
make install-electron     # → ~/.local/bin/Splitty-Electron.AppImage + splitty-web.desktop

# Web app in Tauri from source — includes both GUI and server binary
make install-tauri
# macOS: → /Applications/Splitty.app
# Linux: → ~/.local/bin/splitty-gui + splitty + desktop entry

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

CI lints and tests the native app with the workspace on Linux, builds it on
macOS and Windows, and packs the Linux packages (a release build on Ubuntu
22.04, as the release does, with the deb installed afterwards) and the macOS
disk image with the release's scripts. The Windows installer needs the WSL
backend and is built by the release and the **Windows installers** workflow
only. The legacy shells keep their jobs.

Release packages are built on `v*` tags or by manually starting the release
workflow. Ordinary pushes do not publish packages.

## Releases

The shared [release skill](../.agents/skills/release/SKILL.md) is available
as `$release` in Codex and `/release` in Claude Code. It creates
and pushes a version tag, checks the GitHub Actions run, builds the current
host's desktop package, and copies it with a SHA-256 checksum to Downloads.
WSL builds the Windows installer and uses the Windows host's Downloads folder.
The workflow publishes the GitHub release; the skill does not install the app.

Every distributed build needs a version of its own: hosts and the Windows WSL
backend install a server per version. On a remote host, reusing a version can
replace the payload on disk while leaving an existing daemon of that same
version running. One command sets the version in `Cargo.toml`, `Cargo.lock`,
the Tauri and the Electron configuration, commits and tags:

```bash
scripts/release.sh 0.1.2
git push --atomic origin HEAD v0.1.2
```

The pushed tag starts the release workflow. It refuses a tag that does not match
the version files, builds the packages below and publishes the GitHub release
with them and `SHA256SUMS` once all of them have built; a tag with a suffix
(`v0.2.0-rc.1`) becomes a prerelease.

| System | Package |
|---|---|
| Windows x64 | `Splitty_<version>_x64-setup.exe` |
| macOS | `Splitty_<version>_aarch64.dmg`, `Splitty_<version>_x64.dmg` |
| Linux x86_64 | `splitty_<version>_amd64.deb`, `splitty-<version>-1.x86_64.rpm`, `splitty-<version>-linux-x86_64.tar.gz` |

The release also carries the static servers (`splitty-wsl-x86_64.tar.gz`,
`splitty-wsl-aarch64.tar.gz`, `splitty-darwin-<arch>.tar.gz`). The publish job
stops if two files share a name or an expected package is missing. The
Windows workflow builds the web app's installer only when started by hand
with its `web` input.

The Windows installers are per-user setups: run a newer one over an installed
Splitty to update it, and uninstall it from Windows' installed apps. The
**Windows installers** workflow (`windows-tauri.yml`), started by hand, builds
the static servers and the native installer for any commit, as workflow
artifacts; its `web` input adds the web app's installer.

The static servers are built once per run (`server-payloads.yml`); the
Windows, Linux and macOS jobs start from them side by side. The Release
workflow also runs on `main` every week and when started by hand, where it
builds everything and publishes nothing. A tagged run cannot use a
dependency cache another tag saved, so the release jobs only restore caches
saved on `main`: by that weekly run, and for Linux by CI's native package
job. Pull requests restore caches and save none.

For a branch preview without a release tag, set a distinct prerelease such as
`0.2.0-dev.1` in `Cargo.toml`, `crates/splitty-tauri/tauri.conf.json` and
`frontend-electron/package.json`, then run `cargo update --workspace --offline`
and `scripts/release.sh --check v0.2.0-dev.1`. The check does not create a tag.
Commit these files and `Cargo.lock` before building a Windows preview. The
branch name and commit hash do not automatically change the version. Increment
the prerelease number for each subsequent distributed preview; adding build
metadata alone does not make a daemon eligible for an automatic update.
Remote updates follow SemVer precedence: `0.2.0-dev.9` precedes
`0.2.0-dev.10`, and every `0.2.0` prerelease precedes `0.2.0`. The native
identity and session-keeper requirements still apply. An eligible legacy daemon
can be migrated through the app's confirmed update action; otherwise use the
explicit migration described in [CLI documentation](cli.md#splitty-daemon).

Existing `0775` data/config ancestors (for example `~/.local`,
`~/.local/share` and `~/.config` under umask `0002`) are supported unchanged.
Splitty creates its own private subdirectories; it does not chmod shared
parents. The native endpoint and daemon discovery/log paths use pinned
private directory handles and refuse symlink traversal or foreign ownership.
This applies to fresh startup and the confirmed legacy recovery action.

After a successful release run, **Cleanup release artifacts** deletes that
run's Actions artifacts; the files attached to the release remain available.
New builds retain artifacts for at most three days, including failed releases
and standalone Windows builds. Reruns of older tags use the retention settings
from that tag, but successful release reruns are also cleaned up.

On a Windows PC with the Windows Rust toolchain, Node and pnpm, the native
installer also builds locally in a few minutes, from WSL:

```bash
scripts/windows-installer.sh --install
```

It brings a checkout on a Windows drive (`SPLITTY_WINDOWS_CHECKOUT`, by default
`%USERPROFILE%\Code\splitty-windows-preview`, cloned when missing) to the
committed `HEAD`, runs `packaging/windows/build.ps1` there and, with `--install`,
starts the setup. `SPLITTY_WINDOWS_PATH` adds Windows directories to the build's
`PATH`, for a pnpm that is not on it; `SPLITTY_GHOSTTY_VT_DIR` names the Windows
path of a built terminal library (without it the build runs the bootstrap with
Windows Python); `--skip-tests` skips the type check and the Windows unit
tests, and `--web` builds the legacy Tauri app instead. The script prints the
setup's Windows path. Uncommitted changes are not built, and the version is the
one the sources carry.

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

## Version-skew smoke

`crates/splitty-server/tests/version_skew.rs` runs the last release's `splitty`
against the current build, in both directions: their clients and daemons, a
daemon and the keepers the other build started, the CLI in a pane and the
daemon behind its `SPLITTY_SOCK`, and the files one build writes and the other
reads. CI runs it on Linux in the "Rust tests" job, and the release workflow
runs it before it publishes. Locally, on Linux x86_64 with `gh`:

```sh
scripts/testing/fetch-last-release.sh --out /tmp/splitty-last-release > /tmp/splitty-last-release.env
set -a; . /tmp/splitty-last-release.env; set +a
cargo test -p splitty-server --test version_skew -- --nocapture
```

The script takes the newest release that is not a prerelease (`--tag` picks
another), checks the static server against the release's `SHA256SUMS` and
prints `SPLITTY_TEST_RELEASED_BINARY`. Without that variable the tests report
`SKIPPED`; `SPLITTY_TEST_REQUIRE_RELEASED_BINARY=1` makes the omission a
failure. Every process of both builds runs with a cleared environment and a
throwaway `HOME`, so the tests never reach a daemon of the invoking user.

The native local workspace also moves existing terminals: drag a tab label into
another pane to append it as a tab, or onto a pane edge to split there. Escape
cancels a drag. The operation preserves its terminal session; whole-group moves,
tab reordering and moves across windows/screens are outside this increment.

For a mouse-driven native smoke, provide an isolated X11 display (for example
Xvfb), the native library/font environment, and xdotool, then run
`python3 scripts/testing/gpui-pane-drag.py target/debug/splitty-gpui --output /tmp/splitty-pane-drag`.
It exercises tab drops, four split edges and Escape cancellation, verifies the
same shell continues receiving input, saves screenshots, and closes its owned
window/service. It does not measure latency or exercise physical IME input.
