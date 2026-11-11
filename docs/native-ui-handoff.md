# Native app: state and open work

The native client (`crates/splitty-gpui`) is the desktop app that CI builds
and releases publish for Windows, macOS (Apple silicon) and Linux. The web
shells (Tauri, Electron) remain in the repository as legacy apps: they keep
their CI jobs and are released with "Web" in their names. Nothing of the web
client was removed. Builds and packages are described in
[development](development.md#the-native-app); the acceptance records of the
individual features are in the [GPUI runbook](gpui-spike.md).

## Working on it

Read `CLAUDE.md`, this file, [architecture](architecture.md), the existing
`architecture/client-core.c4` and `architecture/gpui.c4` components, and the
selected OpenSpec change before editing. Run `openspec list --json` from the
checkout and confirm its reported root. Build prerequisites and platform setup
are in [development](development.md#running-from-source) and the
[GPUI runbook](gpui-spike.md#build-and-run).

A new checkout needs generated frontend assets even for client-core tests,
because the embedded service includes the server's asset embed. Prepare them
before Rust compilation:

```sh
pnpm install --dir frontend
pnpm --dir frontend build
pnpm install --dir architecture
export GHOSTTY_VT_DIR="$(python3 scripts/bootstrap-libghostty-vt.py)"
```

Install the platform native dependencies listed in the runbook first. The
Ghostty bootstrap verifies pinned source/toolchain archives; do not reuse an
unverified library prefix. Private caches, build outputs, agent skills and local
test screenshots are not prerequisites shipped by Git. Use the repository's
commands rather than paths from an earlier session's temporary environment.
Keep temporary fixture paths short enough for Unix sockets.

## Implemented and verified (checkpoint 2026-11-08)

- Real local terminals, Screens, splits, tabs, pane movement and existing local
  floating/transfer/template tools; the native model projects the shared mux.
- Drawer navigation, wheel gestures, pin/autohide, Screen placement, transitions,
  live full titles and leading application symbols, notifications and settings.
- Exact-host Drawer navigation/Ports, explicit remote Screen creation with
  host-owned default names, bounded canonical observation and revocable follow.
  Detach preserves host work; reconnect never replays creation.
- One captured local/remote rename editor: double-click/F2/context menu;
  Enter/Blur commit, Escape cancels. Names change only after canonical readback.
  Hover actions have a fixed lane, preserving title hitboxes and a full tooltip.

Current checks: **268 client-core tests, 190 GPUI tests, 17 architecture tests**,
strict touched-crate all-target Clippy, formatting and affected strict OpenSpec
validation at that checkpoint; later feature work added to them. The Linux
platform patch additionally has eight targeted event tests (four added) and
strict platform Clippy. Full commands/evidence are in
[the GPUI acceptance record](gpui-spike.md#completed-rename-and-windows-preview-checkpoint-2026-11-08).

Actual Linux windows prove creation on B while A remains usable during delayed
confirmation, real Detach/Reopen with the same session, and B rename via
Enter/Escape/Blur without changing A's selection or any session/process/SSH
inventory. The fixture uses private `.invalid` hosts and an owned SSH-shape shim.
It is not real OpenSSH installation/update acceptance.

## Architecture boundaries and recent regression lessons

- Server mux state, session lifecycle and host launch defaults remain canonical.
  `splitty-client-core` owns UI-free source collections, transport/runtime policy
  and bounded resource admission; `splitty-gpui` owns interaction/projection.
  The web frontend still has TypeScript orchestration; no WASM client-core
  rollout or web cleanup was performed.
- Keep logical host-group interest separate from current execution authority.
  Capture window owner, source/connection and Screen identity in callbacks;
  aliases, selection and raw IDs cannot retarget them. Default remote close is
  Detach; assessed Terminate is separate. Never replay input or mutations.
- New remote commands must pass **both** outer worker source admission and
  remote dispatch. `RenameScreen` was accidentally still local-only at ingress;
  `validate_worker_command_source` now supplies that same gate to the regression
  and production worker. A driver-only test had missed it.
- Rename target validity survives a transient mutation barrier so editor input
  stays intact; start/commit use stricter admission. Active-platform-window
  state is required for a meaningful GPUI Blur test. Background readback must
  not steal editor or terminal focus.
- Linux X11 previously drained native events indefinitely and starved foreground
  updates, filling both bounded queues and rejecting input. The explicit pinned
  [platform patch](../vendor/gpui-pre-linux/SPLITTY-PATCH.md) bounds raw batches
  and schedules one woken weak continuation, preserving repeat/XIM ordering.
  Keep its version/archive/file provenance and Apache license. Do not replace
  this with larger application queues, input replay or hidden registry edits.
- The Ghostty engine boundary remains separate from GPUI rendering. Existing
  web/xterm workarounds are not automatically native feature requirements.

## Packages, CI and release

- Linux: `packaging/linux/native/build.sh` packs a deb, an rpm and a tar.gz
  (x86_64) with the executable, the `splitty` command and `shell-integration/`
  in `/opt/splitty`. macOS: `packaging/macos/native/build.sh` builds the ad-hoc
  signed `Splitty.app` (`com.splitty.native`) and its disk image for the
  host's architecture. Windows: `packaging/windows/native/bundle.ps1` packs the
  executable and the WSL backend archive into a per-user NSIS installer that
  replaces the web app of earlier releases in place; the app starts without
  arguments and installs its backend into the chosen distribution.
- CI lints and tests the app with the workspace on Linux, builds it on macOS
  and Windows, packs the Linux packages on Ubuntu 22.04 (installing the deb)
  and packs the macOS disk image. The release workflow publishes the native
  packages under the plain names and the legacy ones as `Splitty-Web…` /
  `splitty-web…`; the table is in [development](development.md#releases).
- The Windows preview folders with a launcher that named a distribution, a
  backend path and a configuration directory are superseded by the installer.
  The explicit `--wsl-*` arguments remain for isolated previews and tests
  ([runbook](gpui-spike.md#native-windows-preview)).

## Open work

- macOS runs and the chrome follows the web client's look; the gaps and
  deliberate differences are listed in
  [the GPUI runbook](gpui-spike.md#macos-preview-and-web-look-alignment-2026-11-09).
- `native-remote-screen-create`: **13/14 tasks**; only deferred full workspace
  checks remain. `native-remote-screen-rename`: **9/10 tasks**; same gate remains.
  Do not archive either as fully checked yet.
- The broader `native-remote-screens` change retains separate unfinished
  obligations. Its checklist is not superseded wholesale by these two changes.
  Inspect source and each task before accepting additional parity claims.
- Packaging: the macOS disk image and the Windows installer are built only on
  their CI runners; watch the first CI and release runs. The app sets no
  window `app_id`, so Linux desktops cannot yet match its windows to
  `splitty.desktop` (no `StartupWMClass`). CI compiles the app's tests on
  Windows and macOS but runs them on Linux only. There is no native AppImage,
  no native package for Intel Macs or aarch64 Linux, and the macOS app's
  minimum system version is whatever the build's linker targets; it is tested
  on the release runner's macOS only. The provisional packaging decisions are
  NDD-1 to NDD-6 in `openspec/open-decisions.md`.
- Vendored platform tests are workspace-excluded and require their explicit
  documented regression command.
- Real SSH daemon installation/update, individual Windows remote UX, broader
  terminal/IME/image parity and remaining remote-only/local-only capabilities
  still need their own acceptance. Local profiles/templates/transfers are not
  implicitly remote features. Native host-group reorder UI and remembered
  reference persistence remain unfinished.
- The web client stays usable as the legacy app; there is no approved
  requirement to remove web bridges or adopt full WASM.

Start the next feature only with a fresh scope decision; avoid treating old
planning ideas or every unchecked task as an instruction to implement all of
them automatically.
