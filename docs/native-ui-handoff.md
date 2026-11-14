# Native app: state and open work

The native client (`crates/splitty-gpui`) is the desktop app that CI builds
and releases publish for Windows, macOS (Apple silicon and Intel) and Linux. The
Tauri shell around the web client remains in the repository as a legacy app: it
keeps its CI jobs and is not released. The Electron shell was removed; the web
client itself stays, served by `splitty serve` and shown in the Tauri shell. Builds and packages are described in
[development](development.md#the-native-app); the acceptance records of the
individual features are in the [GPUI runbook](gpui-spike.md).

## Working on it

Read `CLAUDE.md`, this file, [architecture](architecture.md), the existing
`architecture/client-core.c4` and `architecture/gpui.c4` components, and the
selected OpenSpec change before editing. Run `openspec list --json` from the
checkout and confirm its reported root. Build prerequisites and platform setup
are in [development](development.md#running-from-source) and the
[GPUI runbook](gpui-spike.md#build-and-run).

Client-core and the native app build without the frontend: they link the server
without its `web-assets` feature. The workspace tests (`cargo test --workspace`)
also build `splitty-server` itself, which embeds `frontend/dist`, and the
architecture check needs LikeC4. Prepare both before Rust compilation:

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
  The service names new Screens (`screen-names-v1`; the window sends no name)
  and the first Screen of an empty workspace goes out with `onlyIfEmpty`.
- Drawer navigation, wheel gestures, pin/autohide, Screen placement, transitions,
  live full titles and leading application symbols, notifications and settings.
- Exact-host Drawer navigation/Ports, explicit remote Screen creation with
  host-owned default names, bounded canonical observation and revocable follow.
  Detach preserves host work; reconnect never replays creation. The service
  stores a host's Screens when a connection to it becomes ready; while the host
  is connected, the window reports its live catalog whenever it differs from
  the stored Screens (new, renamed, terminated, closed with the last pane) with
  `hub.catalog.observe` (`native_workspace/host_catalog.rs`), so a reconnect
  does not list removed ones while the host is not ready. The report has no
  pane counts; the service keeps its stored ones (`hub-host-state-v1`; an
  older service is sent nothing). Remembered remote Screens are written per
  host (`remoteScreens.updateHost`); an older service is only read.
- Ports: the window shows detected ports, forwards and saved rules and starts
  forwards only on explicit actions. `remote.autoForwardPorts` is the
  service's (`hub-auto-forward-v1`, `splitty_server::hub::auto_forward`); the
  window has no automatic forwarding of its own.
- One captured local/remote rename editor: double-click/F2/context menu;
  Enter/Blur commit, Escape cancels. Names change only after canonical readback.
  Hover actions have a fixed lane, preserving title hitboxes and a full tooltip.
- OSC 52 clipboard writes of a program in a pane (`terminal_engine.rs`, the
  terminal's write callback): live output only, the standard clipboard, at
  most 64 KiB of UTF-8, and only while `terminal.allowOsc52Write` of the
  window's own settings is on, for local and host panes alike. Clipboard
  reads stay unanswered. There is no notice after a copy yet.
- Synchronized output (DEC private mode 2026, `terminal_engine.rs`): between
  a program's `CSI ? 2026 h` and `CSI ? 2026 l` the view paints the frame from
  before the update, so output that arrives in several chunks, as from a
  host, never shows half drawn. The hold ends with the mode's reset, a
  resize, a reset, a scroll or selection in the view, a search query, or one
  second after it began (`SYNCHRONIZED_UPDATE_TIMEOUT` in `terminal.rs`).

- Closing the last pane of a Screen removes the Screen on its server (mux
  reducer, local and host alike); the window drops it through the catalog and
  shows another Screen. For a service of an earlier version, which keeps the
  Screen without panes, `SourceRuntime` sends `close_screen` after the close
  of a pane the window requested settles (`screen_ran_empty`). Two centered
  pages share `workspace/empty_page.rs`: a window without any Screen offers
  "New Screen", and a Screen without panes that the window only observes (a
  process exited on such a service, or another client closed the pane) offers
  "New terminal", which also works on a host Screen. Their look is covered by
  view tests of structure only, not by a rendered image.
- Pane, tab, Screen and window closes and the template review ask the
  service what the close ends (`mux.assess_close`, `close-assessment-v1`)
  before they prompt: `Workspace::assess_then_decide` in `workspace.rs` sends
  `Command::AssessClose`, the source runtime answers with
  `Update::CloseAssessed` (`CloseImpact::Unknown` without the capability, on
  failure or after five seconds), and the prompt is worded from the answer,
  scratchpads and annotations included. The close plan keeps only the
  identity and pending-state checks. View tests answer the assessment with
  `answer_close_assessment`, a fake service built from the model's snapshots.
- The template dialog has no file name field: `template.create` sends the
  name and the service names the file (`template-names-v1`); an existing
  file comes back as `OverwriteRequired` with its name, and the confirmation
  saves the same name with `overwrite`. A local service without the
  capability refuses the save with an update notice.
- Scratchpad editors (`workspace/scratchpad.rs`) save with `ifVersion` from
  `ScratchpadVersion` when the source announces `scratchpad-versions-v1`
  and ignore snapshots below the version their own save is expected to
  become; the two-second hold after a save is left only for services
  without versions.
- Viewer annotations take their anchors and the review from the pane's
  service (`viewer-reviews-v1`): `workspace/viewer/notes.rs` sends
  `Command::AnchorViewer` when the version shown or the annotations' targets
  change and `Command::ReviewViewer` when it copies, sends or answers a
  review; `native_workspace/viewers.rs` answers with `Update::ViewerAnchors`
  and `Update::ViewerReview`. An annotation without an anchor for the
  version shown is listed at its recorded lines without a mark. A host
  without the capability gets a notes panel that asks for its update. The
  notes have no view tests yet.

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

## Startup

Until its workspace arrives, a window shows a startup page
(`workspace/startup_page.rs`, the empty page's column with a spinner): the
phase the start is in, or why it failed and in which phase. On Windows
without WSL arguments `main` starts the WSL preparation
(`startup/wsl.rs::prepare`) on a thread of its own before the window opens,
so WSL boots while the window is created; the workspace worker forwards its
phases (looking for distributions, starting the distribution, installing the
backend) and then reports starting the backend and connecting. A warm start
runs two `wsl.exe` calls one after the other: the inspection and the
backend itself;
listing the distributions (no remembered one) and installing (first start of
a version) add one each. A failure stays on the page instead of ending the
app; closing the window kills a running `wsl.exe` call of the preparation.
The hidden Windows window still waits up to one second for the backend's
appearance before it shows. Not yet seen in a real Windows window; the
provisional choices are NSP-1 to NSP-3 in `openspec/open-decisions.md`.

## Actions that need a newer server

A feature whose operation needs a `mux.welcome` capability is gated in the
command layer of `splitty-client-core`, not hidden in a view: the window
offers the action whatever server a Screen belongs to, and
`native_workspace/capability_gate.rs` maps each such command kind to its
capability. `SourceRuntime::command` keeps the capabilities of the
connection's welcome and, for a command whose capability is missing, sends
nothing, settles the window's pending state with the command's own
rejection and publishes `Update::ServerOutdated`, which the window shows as
its status ("the local service" or "the host's daemon" is older than the app
and needs an update). Only capabilities decide, never a version. A new
feature that needs a newer server adds one row to `required_capability`.
Older checks that change what a command does (a close without
`close-assessment-v1` asks generically, a tab drop without
`tab-placement-v1` is not offered as a target) predate the table and stay
where they are, and so does the template extraction's own check of
`server-templates-v1` in `SourceRuntime::command`, which answers through the
template result rather than a status. A refusal that does come back from a
server (`mux.op_failed`) is an ordinary status message as well
(`Operation rejected: …`, for a transfer `Move rejected: …`). Rows today: the
group drag (`layout-arrange-v1`) and the group transfer
(`tab-group-transfer-v1`).

## Tab groups

The empty area of a tiled group's tab bar, right of its tabs and new-tab
actions, drags the whole group (`render_group_grip` in `workspace.rs`); the
area takes no clicks. The drop targets are a single tab's (`Dragged` in
`workspace.rs`); `workspace/group_move.rs` in client-core turns a drop into
one `arrange_panes` of the root layout the window shows, each group's shown
tab as its active one (TGD-1 to TGD-5 in `openspec/open-decisions.md`).
Floating groups move by their own header. Not yet tried in a running window.

The tab menu's "Move Group to Screen" (groups of more than one tab, tiled or
floating) sends the whole group to another Screen of its server with one
`move_tab_group_to_screen` (`tab-group-transfer-v1`). Client-core captures it
as a `GroupActionTarget` (`workspace/catalog/transfer.rs`) and runs it through
the single tab's transfer path: one pending transfer listing every moved
pane, the same verification and follow to the group's shown tab (TGT-1 to
TGT-5 in `openspec/open-decisions.md`). The menu entry and its submenu were
not tried in a running window either.

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

## Themes

The app resolves UI themes and terminal palettes in its own catalog
(`crates/splitty-gpui/src/appearance/`): GPUI Kit theme sets and Ghostty theme
files, bundled from `crates/splitty-gpui/themes/` and read from the themes
directory. The service stores only the names; which palette name the
terminal settings choose is the server core's rule, shared with the web
client. [Themes](themes.md) describes the
formats and the `splitty` block; `scripts/vendor-themes.py` refreshes the
bundled third-party files. The Splitty themes in `themes/ui/splitty.json` and
`themes/terminal/splitty/` are edited there directly.

## Program status

Each source keeps the root records (OSC 7501) its current connection reports
once its `mux.welcome` listed `program-status-v1`
(`crates/splitty-client-core/src/workspace/program_status.rs`); a host that is
older or not connected shows nothing. The window
(`crates/splitty-gpui/src/workspace/program_status.rs`) draws a pane's state
as a monochrome glyph before the title, on the pane's tab and on its row in the
Drawer: pulsing for a working program, in the warning color for a waiting one,
a check or a cross for a done or failed one; on the tab the state and the
record's message are its tooltip. A program at rest shows a muted glyph there
unless the title starts with an icon of its own, which then stays; a state to
notice takes the place of the title's icon (`title_mark`). The glyphs come from
the terminal face the app carries. A Drawer Screen and a host group caption show
a waiting pane, also for a host's Screens the window has not opened; the host's
panel and the Hosts list word such a Screen as "Waiting". A done or failed mark
goes once the pane is watched, the same rule as for unread notifications, and
returns only for a newer record; what was seen is kept in memory per window.
Reports the server marked `notify` carry the server's notice and become
notifications under `notifications.programStatus` (a row in the notification
settings); the server suppresses the finished-command notices of a session that
holds a record, and bells are no notification. The
outgoing Screen of a transition and a dragged tab show the title's own icon
only, as they show no scratchpad marks.

## SSH questions

Passwords, key passphrases and host key confirmations OpenSSH asks the local
service come to the window over the protocol, the same for the embedded
service and the WSL backend (remote-hosts, SSH Questions Of The Local
Service). The service announces `ssh-prompts-v1` and sends
`ssh.prompts.changed`; the worker
(`crates/splitty-client-core/src/native_workspace/ssh_prompts.rs`) then reads
`ssh.prompts.get` on an API stream of its own, opened at the first
notification, publishes `Update::SshPrompts` and sends
`CommandKind::AnswerSshPrompt` as `ssh.prompts.answer`. A service without the
capability is polled once a second until it refuses a read. The window
(`crates/splitty-gpui/src/askpass.rs`) opens one dialog per question and
closes those whose question went; closing a dialog cancels its question. On
Linux and macOS the app executable is also the askpass helper the embedded
service starts (`embedded::askpass_helper`, first thing in `main`); the WSL
backend's helper is its own `splitty`. Started with `SPLITTY_SSH_ASKPASS=1`,
the askpass call of an earlier version that ran its own dialog, the
executable exits with status 1 and no window (`startup::stale_askpass_call`).
The Windows route has not been run on
Windows yet (SPP-1 in `openspec/open-decisions.md`).

## PTY size

The window follows the servers' size policy (terminal-sessions, Client
Activity Reports and Client Display of the PTY Size). The workspace marks a
key or pointer button, its activation and its resizing, and writes its
visibility and focus, into a cell it shares with the worker
(`crates/splitty-client-core/src/native_workspace/presence.rs`); the worker
greets each `pty-size-v1` welcome with `activity` or `idle` before it attaches,
sends `activity` to the connection of the shown Screen at most every 5 s,
`idle` to every connection once the window is hidden, and nothing of this to a
server without the capability. Each validated `size` reaches the terminal of
its session (`workspace/pty_size.rs`): the emulator runs at that size, the
pane's rest is hatched, a taller grid moves up as far as its cursor row needs,
and a badge with the size takes it over unless the policy is `smallest`. Not
yet seen with two clients on one session in a real window; the provisional
choices are NPS-1 to NPS-3 in `openspec/open-decisions.md`. The worker also
reports the other terminal tabs of a shown pane's tab group at that pane's
measured size and hides them with the group (terminal-sessions, Client Size
Reports; BTS-1 to BTS-3), so a tab switch neither resizes nor redraws.

## Versions

The settings have a read-only About section (`workspace/settings.rs`): the
app's version, the commit of the build where `SPLITTY_COMMIT` was set when it
was compiled (the release and packaging builds), and the version of the
window's local backend once its `build.get` reply arrived. A row whose value
is unknown is left out. The Hosts list and the host's panel show the version
of a connected host's daemon beside its state (`Connected · v0.3.2`). The
client keeps it from the `daemon.version` of the attempt's `remote.ready`
(`HostPresentation::daemon_version` in `splitty-client-core`), so it is known
for every connected host, not only where an update is offered; a host that is
connecting, offline or closing shows none, and so does one whose version was
missing or malformed. Versions of a host or backend are shown as bounded
plain text. The backend's commit and the daemon version of a host whose
connection failed as incompatible are not shown outside the update offer.

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
  packages only; the legacy web apps are no longer released and keep building
  in CI. The table is in [development](development.md#releases).
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
- Windows: the app needs a WSL distribution; hosts without WSL remain with
  the legacy web app built from source (draft `native-windows-without-wsl`). Each start is an
  independent instance with its own backend that shares only the settings
  files with the others (draft `native-windows-one-backend`). The dialog on a
  first start with several distributions and the WSL settings section have
  not been seen in a real window.
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
