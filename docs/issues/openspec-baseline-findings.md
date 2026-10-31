# OpenSpec Baseline Findings (October 2026)

While deriving the baseline specs in `openspec/specs/` from the code, a number of bugs, gaps and stale docs
turned up. The specs only state guarantees the code keeps; everything below was deliberately **left out of
the specs** so a baseline does not cement a bug. Each bug is a candidate for its own OpenSpec change
(`/opsx:propose`), which then adds or modifies the matching requirement.

Line numbers refer to the tree at commit `b495e1c`.

## Bugs and gaps

- **Fixed: the OSC 52 write toggle was not persisted.** The frontend exposed
  `terminal.allowOsc52Write`, but the server settings struct discarded it on PATCH.
  The server now stores and returns the field, with an enabled default and a durable opt-out.

- **Fixed: terminal replies typed into the shell on resume.** Historical output contained OSC color
  queries (notably OSC 11), which the emulator answered again after the querying program had exited.
  The attach router now strips historical terminal report requests before rendering, across frame
  boundaries. Output after `attached` remains live, including while the view is still being wired.
  Both renderers use this path; existing host daemons need no update.

### workspace-model

- **Split sizes are validated against the wrong count.** `split_pane` always expects exactly 2 `resultingSizes`
  (`mux/validate.rs:47`), but when the new slot joins a parent split of the same axis the parent ends up with
  N+1 children (`mux/reducer.rs:296-305`). A correct N+1 array is rejected; a 2-entry array is accepted and leaves
  sizes and children mismatched, which the grid projection silently papers over with equal sizes (`mux/grid.rs:49-53`).
- **Fixed: `move_pane` lost panes.** With `direction` but no `targetSplitId`, an unknown or non-slot `targetSlotId`,
  or a target the move itself removed, the pane dropped out of the layout while the op reported success; moving the
  only pane of a parentless root slot duplicated it. Since `arrange-panes` (2026-10-30) all of these are rejected
  and leave the layout unchanged.
- **A partial `reorder_screens` hides screens.** Screens missing from the list leave the screen order but stay in
  the workspace, so they vanish from `mux.welcome` and listings (`mux/reducer.rs:136-143`, `mux/store.rs:96-108`).
- **Missing validation:** `resultingSizes` on close/move, `sourceResultingSizes` and float bounds are unchecked.
- **A new tab or split off a floating pane is rejected.** `split_pane` needs its pane in the root layout
  (`mux/validate.rs`, `validate_split_pane`), but the client offers new tabs and splits in floating groups. After the
  `mux.op_failed` the client spawns the session itself, so the pane is unknown to the server: gone after a reload
  while its shell keeps running, and every op on it fails. Found on 2026-10-12 while verifying
  `connection-scoped-frontend`; before that change the group header's buttons sent no op at all.
- **Ordinary session status never leaves `starting` in the model** (managed jobs added by `cli-control` do track their lifecycle), and ordinary pane title/cwd/role are never updated after
  creation. The daemon's respawn check for exited sessions (`daemon/mod.rs:108`) therefore never sees live data.
- Pane roles `webview` and `viewer` exist in the model, but no op creates them.

### mux-protocol

- **Layout-only ops never reach other clients.** A WebSocket op that spawns or ends no session sends no nudge
  (`ws/mux_handler.rs:245`), so rename, `resize_split`, `move_pane`, float and `set_active_screen` drift silently
  between clients. Matters as soon as two clients are open (roadmap 101). Exceptions: `move_pane_to_screen`,
  `set_scratchpad` and, since `arrange-panes`, `arrange_panes`, which carries a window's drags and divider moves.
- **Rejected ops are only reported when they carry `newPaneId`** (or are `arrange_panes`, since `arrange-panes`), and an op that fails to parse is dropped before
  `mux.op_failed` even then (`ws/mux_handler.rs:194`); the client waits for its own timeout.
- **`mux.pane_spawned` is sent even when the spawn failed**; effect errors are only logged
  (`ws/mux_handler.rs:250-272`). The client is handed a session ID that does not exist.
- **No backpressure**: each client's outgoing queue is unbounded (`ws/sender.rs:27`).
- **Connect race**: a client joins the broadcast list (`ws/handler.rs:63`) before `mux.welcome` is sent (`:73`).
- Ordering only holds within one op: broadcasts spawn one task per client per batch (`ws/mod.rs`).
- `mux.welcome` does not include the active screen.

### control-api

- **Fixed in `cli-control`: generated screen IDs and spawn results.** HTTP creation returns the actual screen ID, revision and successful pane/session mappings; nudges use that ID. Failed starts return errors and never emit successful spawn events.
- `reorder_screens` cannot be sent over HTTP; it falls through to the screen-op parser and returns 400.
- The embedded Tauri server never removes its control socket file (`splitty-tauri/src/lib.rs:402`); the
  standalone server does (`main.rs:538`, `main.rs:1088`).
- Panes get `SPLITTY_SOCK` even when the socket could not be created (`main.rs:391-394`).
- The frontend calls `/api/config` (`frontend/src/lib/terminal_config.ts:21`) and `/api/themes`
  (`SettingsPanel.svelte:34`); neither route exists.
- **`terminal.allowOsc52Write` is never saved** (found on 2026-10-13 while adding `remote.autoForwardPorts`).
  The server's settings have no such field (`config/settings.rs`, `TerminalSettings`), so `PATCH
  /api/home/settings` drops it and answers without it, and the frontend's merge puts the default back: the toggle
  in the settings panel turns itself off again. It fails safe (terminal clipboard writes stay refused, and so does
  `pane.copy` from a host), but the opt-in cannot be used.

### cli

- `daemon stop` exits 0 when SIGTERM fails (`main.rs:521-523`).
- **Fixed in `cli-control`:** the shared bounded HTTP client preserves non-JSON 401/403 errors, and incomplete explicit address/token pairs fail as usage errors.
- `issue-bootstrap` without a token file creates a fresh token that cannot match the running daemon
  (`main.rs:295`), which ends in a confusing auth failure.
- Daemon liveness is a bare `kill(pid, 0)` (`daemon/mod.rs:73`); after PID reuse a stale info file counts as live.
- `send-keys` cannot send a literal `\n` (no backslash escape, `main.rs:330`).
- **Fixed in `cli-control`:** `split -H` help now says left/right; the default still stacks panes.
- Missing pane/screen targets are sent as empty IDs and rejected by the server instead of failing locally.

### server-modes

- `serve` panics via `unwrap` on a bind failure instead of printing a clean error (`main.rs:496`).
- Because session status never becomes "exited" (see workspace-model), restore respawns sessions whose process had
  already exited while the pane stayed open (`daemon/mod.rs:108`).
- **Windows re-entry hands out a spent bootstrap code**: `windows_wsl.rs:958-967` returns the stored URL whose code
  was already redeemed, and the launcher never sends the `bootstrap` stdin line the server supports
  (contradicting the comment at `main.rs:797-806`).
- `startedAt` / `daemon status` show raw Unix seconds; `chrono_now` claims ISO 8601 (`main.rs:1215`).
- **A macOS host detects no listening ports** (found on 2026-10-19 with `remote-macos-hosts`). The port scan reads
  `/proc` only (`pty/ports.rs`), so a Mac host's caption and Hosts panel offer no detected ports and
  `remote.autoForwardPorts` forwards nothing there; forwarding a port by number works. Candidate fix: `lsof -nP
  -iTCP -sTCP:LISTEN` for the sessions' process trees, as `procinfo.rs` already uses `lsof` for the cwd.
- **Agent sockets are too long under `~/Library/Application Support` on macOS** (found on 2026-10-19 with
  `remote-macos-hosts`). Without `XDG_RUNTIME_DIR`, the desktop app's and a CLI daemon's control directory is
  `~/Library/Application Support/splitty/run/`, and `agent-<uuid>.sock` there exceeds the 104 bytes of a macOS socket
  address for user names longer than about 8 characters, so agent grants fail. A hub's server on a Mac uses
  `~/.config/splitty/run/` and is not affected. Not reproduced yet.
- **The Electron shell's children outlive a killed app** (found on 2026-10-12 while checking `remote-window` by hand).
  When the Electron process is killed by a signal instead of quitting, `will-quit` does not run: its backend
  (`splitty serve --oneshot-token`, without `--desktop-stdio`) and every `splitty connect --json` of a remote window
  keep running, the latter with their tunnels. The Tauri app does not have this problem, because it holds the tunnels
  in-process. Candidate fix: `--desktop-stdio` for the backend, and `connect` ending when its standard input closes.
- **Fixed (2026-10-14): `splitty serve` ignored SIGTERM** (found while checking the remote screens by hand). Only
  Ctrl+C shut it down; a `kill` or a service manager's stop ended the process at once, and the hub's forward `ssh`
  processes lived on, holding their local ports, so the next server forwarded the same host port to another one.
  `serve` now shuts down the same way on SIGTERM (`main.rs`, `shutdown_signal`); SIGKILL still leaves them behind.

### terminal-sessions

- **Links in a terminal do not open in the desktop apps; they have to be copied by hand** (seen 2026-10-14
  on Windows and macOS, cause of plain links not confirmed). Found and fixed the same day: OSC 8 hyperlinks asked
  with `window.confirm`, which the macOS webview (wry 0.57 has no script dialogs there) answers with false, so they
  never opened; a local pane's OSC 8 links took xterm's `window.open()`, which Tauri admits as a blank Splitty
  window; the ghostty-web renderer gave local panes no links at all. All now go through `lib/terminal/hyperlinks.ts`
  and `openExternalTarget`. Plain detected links already took the native opener (`plugin:opener|open_url`, scope
  `http://*`, `https://*`) and work in the browser; a refusal there was only a console warning and is now an error
  notice with the opener's reason. Next: a check in the desktop apps; long URLs that a TUI hard-wraps
  (Claude Code) are still matched only up to the line break.
- **The pane menu's Paste and the `pane.paste` command paste nothing in Chromium, Edge, WebView2 (Windows app) and
  Electron** (verified in Chromium on 2026-10-15): both call `navigator.clipboard.readText()`
  (`lib/context_menus.ts`, `Workspace.svelte`), which the page's own `Permissions-Policy: clipboard-read=()` blocks
  ("The Clipboard API has been blocked because of a permissions policy"). The paste shortcuts work, since they use
  the browser's `paste` event. Options: drop the menu entry and the command, or read the clipboard natively in the
  desktop shells and keep the policy for the browser.
- Notifications (`process.idle`, `process.finished`, `session.exited`) only exist for sessions some connection
  streams: the engine tracks a session from `create`/`attach` on, and the watchers that feed it run per stream
  (`protocol/dispatcher.rs`). Output, title and OSC 133 state are captured without a client, but a session nobody
  streams notifies nobody. Follow-up in `terminal-intelligence`.
- **Fixed in `cli-control`:** the lifecycle owner reports actual exit codes and signals, including when output EOF and process exit arrive in either order.
- `close` does not kill the process, it only hangs up the terminal (SIGHUP to the shell and the foreground process
  group); processes that ignore SIGHUP survive, and until they exit they also keep the session's reader thread,
  PTY and scrollback alive (`pty/session.rs`).
- The pane-to-session lookup for `create` with `paneId` is never wired in production, so such a `create` always
  starts a new random session instead of reusing the pane's (`dispatcher.rs:138-150`).
- `shell_name` claims to be case-insensitive but is not (`pty/startup.rs:75-87`).
- Not specced yet: shell-integration injection (`ZDOTDIR`, `ENV`, `--posix`, `SPLITTY_SHELL_INTEGRATION`,
  `pty/session.rs:509-567`). Belongs to a later terminal-intelligence capability.

### access-control

Hardening candidates, not open holes.

- **The session cookie reaches every server on `127.0.0.1`** (found while planning `remote-screens`).
  Cookies are not port-scoped, so the browser sends `splitty-session` to any loopback server it visits,
  for example a dev server in a Docker container. SR-31 stops *page script* from other ports (Fetch
  Metadata), but such a *server* receives the cookie in the request header and can replay it outside a
  browser: `/api` without `Origin` is allowed, and a non-browser client can set the WebSocket `Origin`
  freely. Locally that is a shell on the PC; through `splitty connect` it is a shell on the remote host.
  Side effect: two Splitty servers in one browser profile log each other out (same cookie name and
  host). Users of the desktop apps are not affected since `remote-window` (2026-10-12): a main window
  never loads another loopback page (navigation guard), and a host opened in a remote window gets a
  cookie store of its own, in memory only, that visits no other server. What remains is the browser path
  (`serve`, `connect`) in a profile shared with other loopback servers; the planned `remote-screens` hub
  avoids remote cookies entirely. Possible mitigation: a second, origin-bound secret (sessionStorage is
  port-scoped) required next to the cookie, a deliberate MODIFIED of `access-control`.
- **Fixed in `remote-window` (2026-10-12): Tauri granted IPC to every `http://127.0.0.1:*` page in the
  main window** (`capabilities/app.json`), including the `wsl_*` commands on Windows, so only the
  navigation guard kept another loopback page out. The main window's capability is now added at runtime
  for the exact origin of its backend (`crates/splitty-tauri/src/capability.rs`, `access-control`,
  Desktop Shell Command Scope), and remote windows are in no capability.

- **The Electron shell has no download guard** (found in `show-saved-download`, 2026-10-31). The Tauri shell
  saves only `blob:` URLs of its backend origin (`crates/splitty-tauri/src/navigation.rs`, `download`);
  Electron's `will-download` (`frontend-electron/src/main.ts`) only remembers what a main window saved and
  refuses nothing. The navigation guard keeps foreign pages out of its windows, so only the backend's own
  pages can start one. Possible fix: cancel every download that is not a `blob:` URL of the window's origin.
- The server never sets `SPLITTY_TOKEN`/`SPLITTY_SERVER` for panes but does not scrub them from its own inherited
  environment either (`pty/session.rs:110-119`).
- The control socket's peer-uid check only exists on Linux; elsewhere only file modes protect it
  (`control.rs:137-144`). SECURITY.md describes the check without that limitation.
- `--token` accepts any value, without a minimum length (`main.rs:286-288`).
- Browser sessions never expire and cannot be revoked short of a restart; the session store is unbounded
  (`http/session.rs`).
- `state.json` and `daemon.log` are written with the default umask, protected only by the 0700 directory
  (`daemon/persistence.rs:23-26`, `main.rs:1017-1021`).
- Binding `[::1]` passes the loopback and Host checks, but the origin allowlist only has `127.0.0.1`/`localhost`
  (`http/mod.rs:61-66`), so the UI gets 403 from its own server. Broken, not insecure.
- Not specced yet, needs a capability decision (likely a future `desktop-shell`): navigation and opener policy of
  the desktop shell, OSC 52 clipboard policy, link scheme checks, template command confirmation, WSL protections
  (non-root default user, credential redaction in logs).

### packaging and releases

- **Fixed (2026-10-15): every build was version 0.1.0.** A host keeps the server of the client's version once
  installed (`~/.local/share/splitty/remote/<version>/`) and only flags a daemon of *another* version, so an updated
  client went on using a host's older server of the same version without a word. The Windows setup also saw no
  newer version and offered only a reinstall. `scripts/release.sh <version>` now sets one version everywhere and
  tags it, and the release workflow refuses a tag that does not match the version files.
- **The Windows release is unsigned, so Smart App Control blocks it** where it is on (2026-10-15). It also blocks
  a local build's build scripts
  ("An Application Control policy has blocked this file", os error 4551). SmartScreen only warns; Smart App Control
  cannot be clicked through, only turned off. Options: the SignPath Foundation (free for open-source projects, needs
  the public repository and a code signing policy page) or Azure Artifact Signing (paid; open to individuals in the
  US and Canada only, to organizations in the EU).
- **A same-version rebuild replaces a host's server but not its running daemon** (found 2026-10-29). A local build
  keeps the version and changes the payload, so the hub reinstalls the host's `remote/<version>/` (moving the old
  directory away and deleting it) and leaves the running, "compatible" daemon alone; Update is offered only for
  another version. That daemon then runs from a deleted file without the new build's features. It also lost the CLI
  on PATH and the shell integration for new shells, fixed in `dba2caa` (the executable is resolved at startup).
  Workaround: `~/.local/share/splitty/remote/<version>/splitty daemon stop` on the host (sessions survive in their
  keepers), then reconnect. Fix candidates: compare the payload hash, not only the version, before calling a daemon
  compatible.
- **Old WSL backends are never removed.** The Windows app unpacks every new archive next to the previous ones in
  `~/.local/share/splitty/desktop/` and only moves the `<version>` link (`windows_wsl.rs`, `provision_backend`);
  four leftovers of 0.1.0 builds were found in one distribution. Uninstalling Splitty leaves them too.

### processes and sockets on macOS

- **A spawned child can hold a port this process just released** (found 2026-10-28). On macOS, Rust sets
  close-on-exec only after `socket()` and `accept()`, and `posix_spawn` copies the parent's descriptors until the
  child executes. A port a thread releases while another thread spawns a process stays busy for tens of
  microseconds, and for the child's whole life when the spawn falls between `socket()` and close-on-exec. Measured:
  357 of 85 385 release-and-rebind attempts failed beside 98 `sh` spawns (held 17 µs to 31 ms), none without
  spawns. Tests that release a port and bind it again at once fail on macOS for this when neighbouring tests spawn:
  `splitty-remote`'s tunnel tests in 10 of 10 parallel runs with
  `a_delayed_ssh_and_competing_ipv4_listener_are_not_forward_readiness` (which spawns `sh`), 1 of 10 without it, and
  the forward tests of `splitty-server`'s `http::tests` alike; all pass with `--test-threads=1`, and in parallel on
  Linux. In production, an `ssh` the hub spawns could, rarely, keep another forward's reservation or a connection
  accepted at that moment open for its lifetime, since the hub's `ssh` children do not close inherited
  descriptors.

## Stale documentation

- **`docs/protocol.ts`** still describes screen ownership, attach/detach, single-attach, `lastClientId`,
  `set_screen_ownership` and the `mux.connect` handshake (lines 11, 25-28, 42, 132-148, 191, 232, 254-261, 460-517,
  624, 666, flows around 723-766), all removed in `fa5e5b7` (March 2026). Op names `split_panel`/`close_panel`/
  `move_panel` are now `split_pane`/`close_pane`/`move_pane`; `float_pane`, `dock_pane`, `move_float`,
  `reorder_screens` are missing. Binary output frames exist (line 9 says "may come later"). It promises 404 for
  not-found on `/api/mux/op` (the code returns 400) and token auth on all endpoints (the control socket has none).
  Until it is fixed, treat the Rust types as the payload reference.
- **`docs/architecture.md`**: the CLI Flow section (lines 67-98) describes `/api/command` round-trips, `focus` and
  `switch-screen`, none of which the CLI uses or has.
- **`roadmap/01-multiplexer-backend.md`** marks ownership, attach/detach, `close`, `focus` and `switch-screen` as done.
- **`docs/protocol.ts`** (sessions): line 183 says `panelId`, the wire field is `paneId`; lines 40-49 claim
  client-owned sessions die with the client; line 426 lists `session.exited` as a session event, but only the
  notification engine sends it (the session end signal is `closed` with `exitCode`).
- **`roadmap/05-persistent-daemon.md`**: `daemon.json` holds no token any more (it has `socket`); the command is
  `splitty daemon …`, not `splitty-server daemon …`.
- **Config paths**: docs say `~/.config/splitty`, the code uses the platform config dir (different on macOS). This
  also affects the token path in SECURITY.md's rotation note.
- **`docs/architecture.md:109`** and README.md:108 say the desktop app connects to a running daemon, which is only
  true on macOS/Linux.
- **README.md:20,51**: daemon sessions "survive restarts". They are respawned as new processes; scrollback is lost.
- **Code comments**: `main.rs:679-682` says a pane process cannot get the token (it can read the 0600 token file,
  as SECURITY.md's threat model states); `daemon/mod.rs:1-5,48-53` still say `daemon.json` holds the token;
  `frontend/src/lib/protocol/http.ts` has a garbled fragment in its header comment.

## Roadmap 106 (CLI Control)

106's agent-oriented job workflow is implemented (change `cli-control`). Its
behavior is recorded in `managed-jobs`, `cli`, `control-api` and the related main
specs. The obsolete `SPLITTY_PORT`/`SPLITTY_TOKEN` discovery is gone; deferred CLI
conveniences are listed in the roadmap, not treated as current guarantees.

## Structure

Found while building the architecture model (`architecture-model`, 2026-10-12); the model in `architecture/` records the current state.

- **The server state is wired twice.** `build_state` in `crates/splitty-server/src/main.rs` and `start_embedded_server`
  in `crates/splitty-tauri/src/lib.rs` both build the template store, the PTY manager, the dispatcher with its
  environment and notification engine, the store, the effect runner, `WsState` and `AppState`, then the control socket
  and the router. The copies already differ: only `build_state` fills in the default shell profile for restored
  sessions and persists the store in daemon mode, which the embedded desktop server does not need today. A function in
  `splitty-server` that builds the state for both composition roots would remove `app`'s relationships into `pty`,
  `protocol`, `effect_runner` and `template`. Not done in `architecture-model` because the desktop startup only runs in
  the app (provisional decision D5 there).

## Untested guarantees

Specced behavior without test coverage, by capability. Good candidates for tests before the next change in
that area touches them.

- **workspace-model:** `move_pane` sizes (`resultingSizes`, `sourceResultingSizes`); successful `float_pane`/`dock_pane`/`move_float`; active-pane fallback
  in a stack; closing the last root pane; closing a floating pane; `newPaneIndex: 0`; `set_active_screen`;
  active-screen fallback on close.
- **mux-protocol:** everything except the binary frame layout: welcome contents, heartbeat, malformed input,
  resync, `pane_spawned`/`op_failed`, nudge propagation and spawn-before-nudge ordering, `command` broadcast,
  disconnect, client reconnect.
- **control-api:** `/api/mux/op` success and rejection at the HTTP level, `/api/command`, `/api/mux/neighbor`,
  `/api/session/{id}/send`, response fields of `/api/mux/screens` and `/api/mux/status`, malformed bodies.
- **cli:** discovery order, no-server error and exit codes, implicit targeting, command output formats,
  `send-keys` escapes, `issue-bootstrap`, the "already running" and stale-file daemon messages. `connect` end to end
  runs only where the SSH lab of `tests/remote_connect.rs` can (an `sshd` and unprivileged namespaces), not in CI.
- **server-modes:** most of it: stale `daemon.json` handling, single instance, `state.json` save/load and
  persist-after-change, respawn with the same IDs, the desktop daemon-or-embedded fallback, the 3-second Windows
  shutdown. Covered: the two `--desktop-stdio` tests and the daemon lifecycle tests (`tests/daemon_lifecycle.rs`:
  start, readiness, `daemon.json` written and removed, missing control socket, startup failure, terminal hangup).
- **terminal-sessions:** real-PTY tests run in CI (`pty/session.rs`, `protocol/dispatcher.rs`): output capture
  without a client, replay and live stream lining up, a busy process, end of stream, `close`, re-attach, reconnect,
  two streams, failed exit status, one exit notification. Untested: environment on the `create` path,
  working-directory resolution, default size, title events, `resize`/`attach` errors, profile fallback,
  `SPLITTY_SCROLLBACK_SIZE`, closing an exited session.
- **remote-screens (saved forwards),** left when `remote-forwards-persistence` was archived (2026-10-28): `lastError`
  cleared by Forget or by a local-port change through PUT, its message and hint and that it carries no credential;
  `preference_limit` over HTTP; the preference order in-process; a PUT queued behind same-key work and the merged
  document of two concurrent PUTs; a failing PUT that edits a label or local port; an alternate port already busy at
  reservation; legacy POST/DELETE with unknown fields and a repeated POST of a forward on an alternate port; the
  router's DELETE status for an active saved rule; for restoration, one entry's SSH error beside others (the scripted
  connector fails globally), an unauthorized setup with saved rules, a PUT while the host is connected, saved port
  and rival listener untouched after a restore conflict, and that no timed retry follows; two requests racing for
  the 32nd slot; the refusal of forwards configured in the SSH configuration, end to end.
- **access-control:** the manual-token path of `POST /api/session`, empty `Host`, mixed-resolution hostnames,
  bind/`--remote` refusal on `daemon start`, session invalidation on restart, `SPLITTY_TOKEN` being ignored,
  the peer-uid check, `daemon start` output, release builds rejecting dev origins, `connect` end to end outside the
  SSH lab (see cli), and all
  UI-side guarantees (bootstrap parameter removed from the URL, fresh ticket per reconnect, nothing in
  `localStorage`).
- **file-transfer,** left when it was archived (2026-10-29): checked end to end once by hand against a local `serve`
  in Chromium (Playwright, one-off: a 1.5 MiB drop, its sanitized name, mode and pasted path; `splitty download`
  saved and dismissed). Untested: the same through the hub to an SSH host, downloads in the desktop shells
  (WebView2, WebKitGTK, WKWebView, Electron), pasting files from the system clipboard, uploads over 10 MiB.

## Flaky tests

Seen on 2026-10-31 in a full `cargo test --workspace` under load (WSL, desktop app running); each passed alone
three times in a row. Both pick local ports that another test running in parallel can take in between.

- `splitty-server` `http::tests::a_host_has_at_most_32_forwards`: one of the fixed ports 41001–41032 got no `200`.
- `splitty-remote` `tunnel::supervisor::tests::forwards_take_the_asked_port_or_any_free_one`: `connect` returned
  fewer than two forwards (`free_port()` releases its reservation before the forward binds).
- `splitty-server` `hub::forwards::tests::ipv6_occupation_blocks_saved_open_and_supervisor_rebuild_without_moving`:
  binding `[::1]` on the port the forward had just used again failed (seen 2026-10-31 under the same load; passed
  in the next run).
- `splitty-core` unit tests for `x86_64-unknown-linux-musl` in release mode (the payload job of the release
  workflow): the test process sometimes dies with SIGSEGV while the `keeper::process` and `pty::session` tests
  spawn processes, in about 1 of 15 runs, already at v0.1.29. The v0.1.30 release run needed a rerun of that job.
  Not seen with the glibc target.
