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

- **Fixed (2026-11-14): a port forward whose `ssh` failed could lose the reason.** A forward or a shared
  port reads `ssh`'s output for its ready line and, when the output ends, read `ssh`'s status once
  (`crates/splitty-remote/src/tunnel/forward.rs`, `tunnel/reverse.rs`). The output of a process ends as it
  exits, a moment before its status can be read, so a refused login or host key was at times reported as
  "ssh ended before the relay reported" (or "SSH ended without confirming its local forward") without `ssh`'s
  own diagnostic and hint. Seen as a failure of `failed_setup_preserves_ssh_diagnostics_after_cleanup` in
  the release run of v0.4.2. Both now wait up to two seconds for the status after the output ended; the
  tests run an `ssh` that closes its output before it exits.

- **Fixed: terminal replies typed into the shell on resume.** Historical output contained OSC color
  queries (notably OSC 11), which the emulator answered again after the querying program had exited.
  The attach router now strips historical terminal report requests before rendering, across frame
  boundaries. Output after `attached` remains live, including while the view is still being wired.
  Both renderers use this path; existing host daemons need no update.

### workspace-model

- **Fixed in `mux-layout-consistency` (2026-11-07): split sizes used the wrong child count.**
  Explicit sizes now match the same-axis parent's resulting child count, or the two-child wrapper
  for a different axis/root slot. Invalid counts and non-finite sizes reject without mutating state
  or producing session effects.
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
- **Fixed in `remove-protocol-leftovers` (2026-11-13): the pane role `webview` existed in the model, but no op
  created it.** A restored `state.json` closes such panes; a window reads the role from an older host as `terminal`.
- A job pane placed as a tab does not become its slot's active pane, unlike `new_tab` and `open_viewer` with `tab`
  (found on 2026-11-06).

### mux-protocol

- **Fixed in `mux-layout-consistency` (2026-11-07): layout-only mutations were not consistently announced.**
  Notifications now follow the changed screen IDs rather than process effects or an operation allowlist,
  with actual added/removed IDs and successful spawn events before one nudge per affected screen.
  The documented window-local `select_tab` exception remains silent.
- **Fixed in `mux-layout-consistency` (2026-11-07): parsed rejected ops were reported selectively.**
  Every parsed dispatch rejection except `select_tab` now reaches its originating client, whose
  frontend resyncs the named screen. Existing malformed/unknown-message policy remains unchanged.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **Workspace directory reconciliation was incomplete between web windows.** Screenless nudges
  now announce order/activation changes, but the web client ignores them. A nudge naming a removed
  screen requests a snapshot that the server intentionally does not send. A directory snapshot/event
  and connection-local reconciliation are separate follow-up work; notification delivery alone does
  not make screen order/removal converge across windows.
- **Resolved: failed spawns no longer announce a nonexistent session.** Only successful
  spawns produce `mux.pane_spawned`; a failed spawn reports `mux.op_failed` to its origin.
  Accepted model changes still notify peers, including when their process effects fail.
- **Stale paths checked (2026-11-15, `remove-browser-access`):** the old `ws/sender.rs`,
  `ws/handler.rs` and `ws/mod.rs` findings describe an obsolete implementation. The retained
  `client_session` and native transport use bounded delivery and ordered welcome/batches;
  removing the gateway does not remove those guarantees.
- `mux.welcome` does not include the active screen.

### control-api

- **Fixed in `cli-control`: generated screen IDs and spawn results.** HTTP creation returns the actual screen ID, revision and successful pane/session mappings; nudges use that ID. Failed starts return errors and never emit successful spawn events.
- `reorder_screens` cannot be sent over HTTP; it falls through to the screen-op parser and returns 400.
- Panes get `SPLITTY_SOCK` even when the socket could not be created (`main.rs:391-394`).
- **`terminal.allowOsc52Write` is never saved** (found on 2026-10-13 while adding `remote.autoForwardPorts`).
  The server's settings have no such field (`config/settings.rs`, `TerminalSettings`), so `PATCH
  /api/home/settings` drops it and answers without it, and the frontend's merge puts the default back: the toggle
  in the settings panel turns itself off again. It fails safe (terminal clipboard writes stay refused, and so does
  `pane.copy` from a host), but the opt-in cannot be used.

### cli

- `daemon stop` exits 0 when SIGTERM fails (`main.rs:521-523`).
- **Fixed in `cli-control`:** the shared bounded HTTP client preserves non-JSON 401/403 errors, and incomplete explicit address/token pairs fail as usage errors.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: `issue-bootstrap` without a token file creates a fresh token that cannot match the running daemon
  (`main.rs:295`), which ends in a confusing auth failure.
- Daemon liveness is a bare `kill(pid, 0)` (`daemon/mod.rs:73`); after PID reuse a stale info file counts as live.
- `send-keys` cannot send a literal `\n` (no backslash escape, `main.rs:330`).
- **Fixed in `cli-control`:** `split -H` help now says left/right; the default still stacks panes.
- Missing pane/screen targets are sent as empty IDs and rejected by the server instead of failing locally.

### server-modes

- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: `serve` panics via `unwrap` on a bind failure instead of printing a clean error (`main.rs:496`).
- Because session status never becomes "exited" (see workspace-model), restore respawns sessions whose process had
  already exited while the pane stayed open (`daemon/mod.rs:108`).
- **Resolved by shell removal (2026-11-15, `remove-tauri-shell`); browser credentials also removed by `remove-browser-access`.** Historical Windows re-entry handed out a spent bootstrap code: `windows_wsl.rs:958-967` returns the stored URL whose code
  was already redeemed, and the launcher never sends the `bootstrap` stdin line the server supports
  (contradicting the comment at `main.rs:797-806`).
- `startedAt` / `daemon status` show raw Unix seconds; `chrono_now` claims ISO 8601 (`main.rs:1215`).
- **A `state.json` with a record this version does not know is dropped as a whole** (found on 2026-11-06 with
  `splitty-open`). `load_state` returns `None` on any parse error (`daemon/persistence.rs`), so a pane role or a
  viewer `source` kind written by a newer version, after a downgrade, starts an empty workspace, and the next
  change overwrites the file. Tolerant variants or a backup of the unreadable file would keep it.
- **A daemon drops the settings a newer version added as soon as it starts** (found on 2026-11-12 by
  `version-skew-smoke-test`; lines as of 0.3.2). The daemon reads `config.toml` at startup
  (`service/runtime.rs:26`), and `config::load` saves it again through the settings this version knows
  (`config/settings.rs:863-874`; `update_at` at `:1002-1011` does the same on every change). After a 0.3.2 daemon
  had set `terminal.theme_dark`, a 0.3.1 daemon started on the same files and nothing else changed, and 0.3.2
  showed the default again. The values the older version knows survive beside an unknown key
  (`tests/version_skew.rs` checks that). Keeping unknown keys through a save, or saving only on a migration or a
  change, would keep them.
- **Debug logs carry client text** (found on 2026-11-06): the mux handler logs every op at debug level, including
  a scratchpad's text and a viewer pane's terminal selection.
- **A macOS host detects no listening ports** (found on 2026-10-19 with `remote-macos-hosts`). The port scan reads
  `/proc` only (`pty/ports.rs`), so a Mac host's caption and Hosts panel offer no detected ports and
  `remote.autoForwardPorts` forwards nothing there; forwarding a port by number works. Candidate fix: `lsof -nP
  -iTCP -sTCP:LISTEN` for the sessions' process trees, as `procinfo.rs` already uses `lsof` for the cwd.
- **Agent sockets are too long under `~/Library/Application Support` on macOS** (found on 2026-10-19 with
  `remote-macos-hosts`). Without `XDG_RUNTIME_DIR`, the desktop app's and a CLI daemon's control directory is
  `~/Library/Application Support/splitty/run/`, and `agent-<uuid>.sock` there exceeds the 104 bytes of a macOS socket
  address for user names longer than about 8 characters, so agent grants fail. A hub's server on a Mac uses
  `~/.config/splitty/run/` and is not affected. Not reproduced yet.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **Fixed (2026-10-14): `splitty serve` ignored SIGTERM** (found while checking the remote screens by hand). Only
  Ctrl+C shut it down; a `kill` or a service manager's stop ended the process at once, and the hub's forward `ssh`
  processes lived on, holding their local ports, so the next server forwarded the same host port to another one.
  `serve` now shuts down the same way on SIGTERM (`main.rs`, `shutdown_signal`); SIGKILL still leaves them behind.

### terminal-sessions

- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **Links in a terminal of the web client had to be copied by hand** (seen 2026-10-14). Found and fixed the
  same day: OSC 8 hyperlinks asked with `window.confirm` and a local pane's OSC 8 links took xterm's
  `window.open()`; the ghostty-web renderer gave local panes no links at all. All now go through
  `lib/terminal/hyperlinks.ts` and `openExternalTarget`. Long URLs that a TUI hard-wraps were
  matched only up to the line break; sessions now start with `FORCE_HYPERLINK=1`, so Claude Code sends them as
  OSC 8 hyperlinks that open whole from every line (Codex sends them without it). Plain text that a program wraps
  itself is still matched per line, and xterm.js underlines only the hovered line of a wrapped hyperlink.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **The pane menu's Paste and the `pane.paste` command paste nothing in Chromium and Edge** (verified in
  Chromium on 2026-10-15): both call `navigator.clipboard.readText()`
  (`lib/context_menus.ts`, `Workspace.svelte`), which the page's own `Permissions-Policy: clipboard-read=()` blocks
  ("The Clipboard API has been blocked because of a permissions policy"). The paste shortcuts work, since they use
  the browser's `paste` event. Option: drop the menu entry and the command.
- Notifications (`process.idle`, `process.finished`, `session.exited`) only exist for sessions some connection
  streams: the engine tracks a session from `create`/`attach` on, and the watchers that feed it run per stream
  (`protocol/dispatcher.rs`). Output, title and OSC 133 state are captured without a client, but a session nobody
  streams notifies nobody. Follow-up in `terminal-intelligence`.
- **Fixed in `program-status`: the BEL that ends an OSC the scanner does not read counted as a bell** (found by
  reading the code on 2026-11-12): for an OSC number other than 0, 2 and 133 the scanner went back to its idle
  state at the `;` (`pty/osc.rs`), so it counted the BEL that ends the sequence as a standalone bell and the
  session's streams got a `bell` event, for every BEL-terminated OSC 7, 8, 9, 777, 7501 or 21337. The same held for
  an OSC number of five digits (OSC 21337) and for a title or OSC 133 payload over its limit. The scanner now reads
  every OSC to its end (BEL, `ESC \`, CAN, SUB, or an ESC that begins another sequence) and never counts the BEL
  that ends one. It also no longer takes the byte 0x9d as an OSC introducer, which in UTF-8 output is part of a
  character (such as `Ý`), so a BEL after such a character is a bell again.
- **Fixed in `cli-control`:** the lifecycle owner reports actual exit codes and signals, including when output EOF and process exit arrive in either order.
- `close` does not kill the process, it only hangs up the terminal (SIGHUP to the shell and the foreground process
  group); processes that ignore SIGHUP survive, and until they exit they also keep the session's reader thread,
  PTY and scrollback alive (`pty/session.rs`).
- The pane-to-session lookup for `create` with `paneId` is never wired in production, so such a `create` always
  starts a new random session instead of reusing the pane's (`dispatcher.rs:138-150`).
- `shell_name` claims to be case-insensitive but is not (`pty/startup.rs:75-87`).
- **Fixed: TUIs such as Claude Code and Codex rendered garbled** (escape sequence tails such as `;153;153;153m`
  shown as text, lines shifted or duplicated) until a resize made them repaint (2026-11-06). Causes, in
  `frontend/src/lib/terminal/` unless named otherwise. The web-only causes below are
  resolved by removal (2026-11-15, `remove-browser-access`); sizing and Rust replay
  fixes remain applicable to native clients and `splitty attach`:
  - panes on an inactive screen took a new size into their emulator without reporting it, and a resize overtook
    output still batched or still in xterm.js's parser (`terminal_session.ts`, `ordered_output.ts`);
  - the synchronized update (DEC 2026) around each frame's batch landed inside a sequence that the frame boundary
    split, or right before a REP, and its ESC[?2026l ended a synchronized update of the program's own;
  - xterm.js 6.0 drops a character whose write ends after its bytes E2 80 (`…`, `—`, `’`), also at every 128 KiB of
    a larger write; batches now end between characters;
  - the reset written when the alternate screen ends came after the batch being parsed, possibly inside a sequence;
  - the replay's end cancelled with CAN a sequence that the live output went on with (`replay_output.ts`, and
    `pty/replay.rs` for `splitty attach`);
  - widths came from the graphemes addon's plain "15" provider, which gives combining marks and the emoji variation
    selector a column of their own (`⚠️` took three columns, the programs count two), and were set only after the
    addons had loaded asynchronously;
  - the emulator took a reported size before the server answered, so output written for the old size was parsed at
    the new one;
  - once the size arbiter had forgotten a session (no connection reports a size for it or streams it, as after a
    reconnect or a new window), the report after an attach set the PTY to the size it had, so `redraw: true` made
    nobody redraw; it now nudges then (`protocol/sizing.rs`);
  - a failed resize counted as applied and was published, and the same report was skipped later; the PTY now keeps
    its size and the arbiter tries again (`protocol/sizing.rs`);
  - the replay filters did not end a sequence at CAN, SUB or an ESC, so a query after a sequence that never ended
    got through, and a cancelled clipboard write swallowed the output after it (`replay_output.ts`,
    `pty/replay.rs`).
- Still open from the same review:
  - **No `size` before the replay of a session the size arbiter forgot** (`prune` in `protocol/sizing.rs`): the
    replay is parsed at the pane's size, not the PTY's. The redraw after the attach repaints full-screen programs;
    plain output stays wrapped at the pane's width. Needs the PTY's size from the session.
  - **Size events are not ordered with the output.** The server resizes the PTY, publishes the size and streams
    output on separate tasks (`protocol/sizing.rs`, `watch_size` and `stream_output` in `protocol/dispatcher.rs`),
    and the `watch` channel coalesces sizes, so a client can parse a repaint at the old size or old output at the
    new one.
  - **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **A reattach can show old live frames before the replay.** Frames sent before the server handled `detach` that
    arrive after the new `attach` count as replay (`attach_registry.ts`), so the fresh emulator gets them twice, the
    first copy possibly starting inside a sequence. Happens when a snapshot rebuilds a screen while a session
    writes; a fix needs a marker for the replay's start.
  - **The replay starts wherever the ring cut**, possibly inside a sequence (`pty/ringbuffer.rs`). Since
    `replay-terminal-modes` the server restores the input, cursor and screen modes ahead of it (`pty/modes.rs`),
    but not the screen contents, scrolling region, tab stops, character sets or colors set before it. After a daemon
    restart modes older than the keeper's buffer are lost, and `splitty attach` straight to a keeper gets no restore
    (RTM-1).
  - **Fixed (2026-11-12): the replay filters kept some report requests.** `csi_has_side_effect` in
    `pty/replay.rs`, and `replay_output.ts` with the same rules, now also drop the kitty keyboard query `CSI ? u`,
    XTVERSION (`CSI > q`) and the cell size report `CSI 16 t`, which programs send at startup; replayed under
    `splitty attach` they made the user's terminal type its answers into the session.
  - **`splitty attach` pops one kitty keyboard level on exit**: its reset sends `CSI < u` once
    (`cli/attach/terminal.rs`), so a program that pushed several levels leaves the outer terminal with the rest.
  - **XTSAVE and XTRESTORE are not tracked** (`CSI ? Pm s`, `CSI ? Pm r`): the mode tracker (`pty/modes.rs`)
    ignores them, so it misses a mode change made through XTRESTORE, and a replay starts with that mode as it was
    before the change.
  - **Keeper recovery continues across lost bytes**: `lost: true` is only logged, and the remaining bytes go to the
    running streams (`keeper/client.rs`).
  - **A keeper replay above 8 MiB is cut silently** (`install` and `MAX_BEHIND` in `keeper/process.rs`), only with a
    scrollback size above that.
  - **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **Output after `created` can be lost** when the native IPC delivers it in the same chunk as `created`: the view
    is wired only after the awaited promise (`createTerminalSession` in `terminal_mount.ts`). Affects the direct
    spawn path, not panes the server spawns.
  - **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **Output is credited before xterm.js parses it**; past xterm.js's 50 MB write buffer limit a write throws and
    its batch is lost (`native_socket.ts`, `ordered_output.ts`).
- Not specced yet: shell-integration injection (`ZDOTDIR`, the Bash startup bootstrap and
  `SPLITTY_SHELL_INTEGRATION` in `pty/session.rs`). Belongs to a later terminal-intelligence capability.

### access-control

Hardening candidates, not open holes.

- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **The session cookie reaches every server on `127.0.0.1`** (found while planning `remote-screens`).
  Cookies are not port-scoped, so the browser sends `splitty-session` to any loopback server it visits,
  for example a dev server in a Docker container. SR-31 stops *page script* from other ports (Fetch
  Metadata), but such a *server* receives the cookie in the request header and can replay it outside a
  browser: `/api` without `Origin` is allowed, and a non-browser client can set the WebSocket `Origin`
  freely. Locally that is a shell on the PC; through `splitty connect` it is a shell on the remote host.
  Side effect: two Splitty servers in one browser profile log each other out (same cookie name and
  host). The native app is not affected: it uses no browser credentials. What remains is the browser path
  (`serve`, `connect`) in a profile shared with other loopback servers; the planned `remote-screens` hub
  avoids remote cookies entirely. Possible mitigation: a second, origin-bound secret (sessionStorage is
  port-scoped) required next to the cookie, a deliberate MODIFIED of `access-control`.

- The server never sets `SPLITTY_TOKEN`/`SPLITTY_SERVER` for panes but does not scrub them from its own inherited
  environment either (`pty/session.rs:110-119`).
- The control socket's peer-uid check only exists on Linux; elsewhere only file modes protect it
  (`control.rs:137-144`). SECURITY.md describes the check without that limitation.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: `--token` accepts any value, without a minimum length (`main.rs:286-288`).
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: Browser sessions never expire and cannot be revoked short of a restart; the session store is unbounded
  (`http/session.rs`).
- `state.json` and `daemon.log` are written with the default umask, protected only by the 0700 directory
  (`daemon/persistence.rs:23-26`, `main.rs:1017-1021`).
- `grant_command_allowed` lets layout commands through to grant sockets whose routes answer 404 there, so an agent
  under a grant gets a not-found error instead of a refusal (found on 2026-11-06).
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: Binding `[::1]` passes the loopback and Host checks, but the origin allowlist only has `127.0.0.1`/`localhost`
  (`http/mod.rs:61-66`), so the UI gets 403 from its own server. Broken, not insecure.
- Not specced yet, needs a capability decision (likely a future capability of the native app): OSC 52 clipboard
  policy, link scheme checks, template command confirmation, WSL protections (non-root default user, credential
  redaction in logs).
- **An invalid host target over the owner connection has no `remote.error`** (found 2026-11-15,
  `remove-browser-access`). SSH Target Validation specifies `remote.error` with code `invalid_target`.
  A daemon's client endpoint answers so; the owner backend (`native/helper/owner.rs`) closes the
  stream with the plain message "invalid owner host target". No SSH starts either way, and the
  desktop app validates targets before it opens a stream.

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
- **A local Windows setup builds its four server payloads one after another** (found on 2026-11-14).
  `packaging/windows/build.ps1` runs `build-wsl-payload.sh` for `aarch64`, `darwin-aarch64` and
  `darwin-x86_64` and then for `x86_64`, each a release build of the workspace's own crates of about a minute;
  with the Windows app that is most of the six to seven minutes of a setup. A release changes the version of
  every crate, so none of it is reused from the build before. The first three do not depend on each other and
  could run at once (each target has its own directory under `target/`), and a switch that leaves out the
  payloads a machine's hosts do not need would shorten a local build further; a host of a left-out system
  would then need the release's setup.

### processes and sockets on macOS

- **Apple Bash does not restore the Splitty CLI path through the current startup integration**
  (reproduced 2026-11-03 on baseline `cc66e97`). The actual macOS
  `cli_path_survives_bash_and_zsh_startup_without_editing_user_files` test runs the
  user's Bash profile but cannot find `__splitty_restore_cli_path` or the CLI.
  The same failure occurs on the native-transport branch. The startup code and
  test are unchanged. Apple's Bash 3.2 startup code shadows `posixly_correct`
  inside the interactive startup block, bypassing the `ENV` integration path
  used with `--posix` ([Apple Bash source](https://github.com/apple-oss-distributions/bash/blob/main/bash-3.2/shell.c#L1041)).
  Fixed by the separate startup bootstrap in `50470d1`, preserving login-shell
  state and profile selection without POSIX mode. Actual macOS CI run
  41734365955 passes the CLI-path, environment and history regressions.
- **The final-output PTY test assumes a background writer survives session-leader exit**
  (reproduced 2026-11-03 on baseline `cc66e97`). On macOS,
  `logs_keep_final_bytes_when_the_process_exits_before_capture_ends` already sees
  output capture complete before the fixture releases its background writer.
  The same assertion fails on the native-transport branch. Darwin revokes the
  controlling terminal when its session leader exits, independently of whether
  the background writer ignores SIGHUP ([Darwin exit source](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_exit.c#L2140)).
  Fixed in `bff1069` by separating deterministic process-result-before-reader-EOF
  ordering from real PTY final-byte capture before exit. Both cases pass on
  actual macOS in run 41734365955. The production reader still respects real EOF;
  the earlier failure did not establish a product loss-of-final-bytes regression.

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

### native app

- **Fixed (2026-11-14): a replay before a view's first layout was parsed at 80×24.** A server that decides
  sizes sends a session's PTY size before its replay, and the terminal view takes that grid so the replay is
  read at the size it was written at. A view without a layout yet only stored the size (`set_pty_size`,
  `crates/splitty-gpui/src/terminal.rs`): a new view starts at 80×24 and a view reset for a reconnect keeps
  its old grid, and the output that streams before the layout, as for a host's Screen that reconnects while
  another Screen is shown, was parsed at that grid and rewrapped afterwards. Programs that draw with cursor
  movement showed scattered text. The view now takes the PTY's grid at once; the first layout still sizes it
  and reports the pane's size (`a_pty_size_reaches_the_emulator_before_the_first_layout`). A server without
  `pty-size-v1` sends no size, so its replay is still parsed at the view's own grid.
- **A pane shown again is first reported at its size from before it was hidden** (found on 2026-11-14, read
  in the code, not observed). The native worker keeps a session's last measured size while the session stays
  routed (`sizes`, `crates/splitty-client-core/src/native_workspace/source.rs`) and `reconcile` reports a
  shown pane as soon as it has one. A pane hidden with its Screen or behind a maximized slot and shown again
  after the window changed size is reported at the old size until the window measures it, so its PTY can
  change size twice. Dropping the measured size with the `hide`, as a route refresh does, would leave the
  first report to the measurement. The background tabs of a shown tab group are not affected: they follow
  the shown tab's size and are not hidden.
- **Fixed (2026-11-12): a scratchpad on a host's Screen was never saved and lost its text at once.** The native
  window's gate for commands to a host (`Remotes::command`, `native_workspace/remote.rs`) did not list
  `SetScratchpad`, so every save from a pane of a remote Screen was refused before it reached the host; the
  refusal went to the host's diagnostic, not to the window's status line. The text lived in the editor alone
  until the next workspace update more than two seconds after the last save, when the window took the Screen's
  snapshot, which still held the old text, in its place (`reconcile_scratchpads`,
  `crates/splitty-gpui/src/workspace/scratchpad.rs`). The gate now admits the save; an in-process test sends
  one through the gate to a service and reads it back from the snapshot
  (`a_scratchpad_save_on_a_remote_screen_reaches_its_host`). Local Screens were not affected.
- **A scratchpad save that is not applied still loses its text** (found on 2026-11-12). The native window clears
  an editor's unsaved mark when it hands the save to its worker (`flush_scratchpad`) and never learns whether
  the save was applied. A save refused later (a close in progress, a replaced connection, a full command
  queue, a host that rejects the op) leaves text that the next snapshot replaces after the two seconds in
  which a saved text outlasts older snapshots. Keeping the mark until a snapshot shows the saved text, or taking
  a snapshot's text only when it differs from the last one taken, would keep the edit.
- **The native window offers scratchpads without `scratchpad-v1`** (found on 2026-11-12). `session-scratchpad`
  lets a client offer a server's scratchpads only when the server announced the capability; the native window
  does not read it, so on a host whose daemon does not know `set_scratchpad` the text is typed, never kept,
  and replaced by the next snapshot.
- **The native zoom keeps a built-in range** (found on 2026-11-13). Settings edits follow the server's
  metadata, but the window shows a stored `appearance.zoom` outside 0.5 to 3.0 at 1.0, and its zoom keys step
  only through the built-in steps up to 3.0 (`Zoom::from_values`, `Zoom::apply`,
  `crates/splitty-client-core/src/zoom.rs`). A server that widens the zoom limit would store a level the
  window does not show. The current server's limit is the same range.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **A CLI in a pane of the desktop app no longer asks SSH questions** (found on 2026-11-13). A service asks
  its SSH questions through its clients (`ssh-prompts-v1`), and the fallback to `SPLITTY_ASKPASS_PROGRAM` is gone
  from `crates/splitty-remote/src/ssh.rs`, with the variable the app set for the processes it started. A
  `splitty` process that runs `ssh` itself, such as `splitty connect <host>` typed in a pane, has no askpass
  program and keeps `BatchMode=yes`: a host that needs a password, a passphrase or a host-key confirmation
  fails with the authentication hint where the app's question window appeared before. Connecting the host from
  the app asks as before. Letting such a CLI hand its questions to the service it runs under, or asking on its
  own terminal when it has one, would restore it.
- **The hint after a cancelled or refused SSH answer says Splitty never asks** (found on 2026-11-13). A host
  open that fails on authentication reports "Splitty logs in without prompts. Check that `ssh <host>` logs in
  with a key … without asking for a password." (`crates/splitty-server/src/hub/failure.rs`). That holds for a
  CLI without a question broker; a service that asked and got no answer, or a wrong one, shows the same hint.
- **A download keeps a name Windows reserves** (found on 2026-11-13). `download_name`
  (`crates/splitty-client-core/src/native_workspace/files.rs`) replaces the characters Windows refuses but
  keeps device names (`CON`, `NUL`, `COM1`, with or without an extension) and trailing dots and spaces; saving
  such an offered file fails on Windows with the system's error instead of under an adjusted name.
- **The native app does not adopt the login shell's `PATH`** (found on 2026-11-15 in `remove-tauri-shell`, RTS-3).
  Started from the Dock, the Finder or a Linux desktop launcher, the app has the launcher's `PATH` (macOS:
  launchd's `/usr/bin:/bin:/usr/sbin:/sbin`), and so has everything it starts without a shell: `ssh` with a
  `ProxyCommand` from the user's SSH configuration (such as `gcloud` from `/opt/homebrew/bin`) fails, and the
  embedded server's managed jobs miss the user's tools. Terminals are not affected; their login shells set their
  own. The earlier web app's shell took `PATH` from `$SHELL -l -i -c` at startup (at most 5 s, `PATH` only); that
  code (`login_path.rs`) is in the history of the `remove-tauri-shell` commit. The requirement `server-modes`,
  Desktop App Command Search Path, went with that shell; moving the adoption into the native app is a change of
  its own.

## Stale documentation

- **`README.md` has no screenshots** (2026-11-15, `remove-browser-access`): the earlier ones showed the
  web client and went with it; captures of the desktop app are missing.
- **`terminal-sessions` has a contradictory profile-failure scenario** (found 2026-11-07 during
  `native-shell-profile-picker`): Session Creation Failure says a disabled/unavailable requested
  profile always fails, while Shell Program Selection and the current resolver correctly specify
  fallback to an available default. The native picker follows the resolver and reports the
  effective profile/fallback reason. Reconcile the older failure scenario separately; failure
  remains correct when no fallback can start.

- **`docs/protocol.ts` transport and flow text corrected (2026-11-15, `remove-browser-access`).**
  The header and sections 5–6 no longer claim a WebSocket handshake, single-attach flows,
  client-owned layout dispatch or token auth. Native text/binary framing is documented.
  Payload declarations outside the browser-removal list were deliberately retained: the older
  ownership, `lastClientId`, `set_screen_ownership` and `mux.connect` shapes and any mismatches
  with Rust types still need a separate wire-reference audit. Rust types remain authoritative.
- **Resolved by documentation rewrite (2026-11-15, `remove-browser-access`).** Historical `docs/architecture.md`: the CLI Flow section (lines 67-98) describes `/api/command` round-trips, `focus` and
  `switch-screen`, none of which the CLI uses or has.
- **`roadmap/01-multiplexer-backend.md`** marks ownership, attach/detach, `close`, `focus` and `switch-screen` as done.
- **`docs/protocol.ts`** (sessions): line 183 says `panelId`, the wire field is `paneId`; the legacy ownership payload remains
  (its lifetime comment was corrected on 2026-11-15 by `remove-browser-access`); line 426 lists `session.exited` as a session event, but only the
  notification engine sends it (the session end signal is `closed` with `exitCode`).
- **`roadmap/05-persistent-daemon.md`**: `daemon.json` holds no token any more (it has `socket`); the command is
  `splitty daemon …`, not `splitty-server daemon …`.
- **Config paths**: docs say `~/.config/splitty`, the code uses the platform config dir (different on macOS). This
  no longer affects a token rotation note: that note was removed with browser credentials
  (2026-11-15, `remove-browser-access`).
- **`docs/architecture.md:109`** and README.md:108 say the desktop app connects to a running daemon, which is only
  true on macOS/Linux.
- **README.md:20,51**: daemon sessions "survive restarts". They are respawned as new processes; scrollback is lost.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **Code comments**: `main.rs:679-682` says a pane process cannot get the token (it can read the 0600 token file,
  as SECURITY.md's threat model states); `daemon/mod.rs:1-5,48-53` still say `daemon.json` holds the token;
  `frontend/src/lib/protocol/http.ts` has a garbled fragment in its header comment.

## Roadmap 106 (CLI Control)

106's agent-oriented job workflow is implemented (change `cli-control`). Its
behavior is recorded in `managed-jobs`, `cli`, `control-api` and the related main
specs. The obsolete `SPLITTY_PORT`/`SPLITTY_TOKEN` discovery is gone; deferred CLI
conveniences are listed in the roadmap, not treated as current guarantees.

## Untested guarantees

Specced behavior without test coverage, by capability. Good candidates for tests before the next change in
that area touches them.

- **workspace-model:** `move_pane` sizes (`resultingSizes`, `sourceResultingSizes`); successful `float_pane`/`dock_pane`/`move_float`; active-pane fallback
  in a stack; closing the last root pane; closing a floating pane; `newPaneIndex: 0`; `set_active_screen`;
  active-screen fallback on close.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **web frontend:** `RenderedDocument`'s fallback for engines without `adoptedStyleSheets` (the cleaned sheet
  serialized into a `<style>` of the shadow root and parsed again) has no test.
- **mux-protocol:** everything except the binary frame layout: welcome contents, heartbeat, malformed input,
  resync, `pane_spawned`/`op_failed`, nudge propagation and spawn-before-nudge ordering, `command` broadcast,
  disconnect, client reconnect.
- **control-api:** `/api/mux/op` success and rejection at the HTTP level, `/api/command`, `/api/mux/neighbor`,
  `/api/session/{id}/send`, response fields of `/api/mux/screens` and `/api/mux/status`, malformed bodies.
- **cli:** discovery order, no-server error and exit codes, implicit targeting, command output formats,
  `send-keys` escapes, the "already running" and stale-file daemon messages. The browser-command
  coverage gap is resolved by removal (2026-11-15, `remove-browser-access`); the hub
  end-to-end checks still require the SSH lab of `tests/remote_connect.rs`.
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
- **access-control:** browser authentication, origin, cookie, bootstrap, ticket and development-origin
  coverage gaps are resolved by removal (2026-11-15, `remove-browser-access`). The remaining
  surfaces still need their own coverage: owner/grant/keeper peer-UID checks, native endpoint
  identity, daemon startup output and hub setup outside the SSH lab. Removal alone proves none
  of these retained guarantees.
- **file-transfer:** the browser acceptance record below is historical after removal
  (2026-11-15, `remove-browser-access`); native file transfers still need their own acceptance.
  When archived (2026-10-29), checked end to end once by hand against a local `serve`
  in Chromium (Playwright, one-off: a 1.5 MiB drop, its sanitized name, mode and pasted path; `splitty download`
  saved and dismissed). Untested: the same through the hub to an SSH host, pasting files from the system
  clipboard, uploads over 10 MiB.
- **version skew** (noted 2026-11-12, narrowed by `version-skew-smoke-test` the same day):
  `tests/version_skew.rs` runs the last release's static Linux x86_64 `splitty` (fetched and checked against the
  release's `SHA256SUMS` by `scripts/testing/fetch-last-release.sh`) against the current build, in CI on Linux and
  in the release workflow before it publishes. In both directions it covers the CLI commands, `attach` and
  `daemon stop` of one build against the other's daemon, and the current native client through either build's
  `remote-bridge`; a keeper controlled without a daemon and adopted by the other build's daemon; the other build's
  CLI in a pane over its `SPLITTY_SOCK`, before and after the other build's daemon took over; and `state.json`
  (kept and ended sessions), `config.toml` and `daemon.json`. Each creates, attaches, resizes and re-attaches a
  session. Its first run (0.3.1 against 0.3.2) found the dropped settings under server-modes. Still untested across
  versions: the desktop window as a peer (the native app's client core, its capability fallbacks and layout ops) and
  any older client of a newer daemon beyond the `splitty` command; everything over SSH (the hub reading
  the other build's `remote-handshake`, installing or updating a host's daemon, `remote-migrate`); managed jobs,
  agent grants, viewer panes, annotations and scratchpads in `state.json`, and the other files below the
  configuration directory (templates, themes); the macOS and Windows (WSL) builds; versions older
  than the last release (run once by hand against 0.2.4 on 2026-11-12: passed). The pinned rollback smoke in
  `tests/daemon_lifecycle.rs` needs Bubblewrap and does not run in CI. A run shows only what differs between the
  two builds: 0.3.1 and 0.3.2 differ in the server code by two settings.
- **native app on Windows:** the startup failure of a machine without a WSL 2 distribution (`server-modes`,
  Desktop App Hosting On Windows, "Remote only") is covered by the unit test of the candidate selection in
  `crates/splitty-gpui/src/startup/wsl.rs` only; the dialog itself has not been checked on such a machine.

## Flaky tests

- Seen on 2026-11-14 in CI on `main` for the v0.4.3 version commit, in the Rust tests job, one in each of two
  attempts; neither is investigated:
  `native_workspace::ssh_prompts::tests::a_question_of_the_local_service_reaches_the_window_and_its_answer_the_helper`
  (`splitty-client-core`) waited ten seconds for the worker's question list ("question update: Elapsed"), and
  `hub::reverse::tests::stop_pauses_a_saved_rule_and_forget_keeps_the_forward` (`splitty-server`) could not
  bind the shared port right after `stop` ("the host's port is free"). The release run of the same commit
  passed both.
- Seen on 2026-11-14 in CI on `main` for the v0.4.1 version commit, in the macOS job:
  `pty::session::tests::managed_stop_escalates_and_retains_final_output` (`splitty-core`), where the first of two
  stops at once failed with "killpg owned leader group … with SIGKILL: EPERM". Darwin answers `killpg` with
  EPERM once the leader is a zombie; `group_has_only_exited_leader` (`pty/lifecycle.rs`) accepts that only for a
  complete, sole-leader group. Not investigated.
- **Fixed (2026-11-14): the role-catalog test compared another test's SSH question.** Seen once, in the static
  release build (`framed_native_welcomes_before_replies_and_enforces_the_fixed_role_catalog`,
  `crates/splitty-server/src/http/tests/conformance.rs`): its snapshot of the local management state read
  `ssh.prompts.get`, the process's questions, while the test beside it had one waiting. The snapshot leaves the
  questions out.
- **Fixed (2026-11-13): the askpass change-feed test read another test's notification.** Seen once, in the
  release run of v0.4.0 (`askpass::tests::publishing_and_settling_a_question_move_the_change_feed`,
  `crates/splitty-remote/src/askpass.rs`): the change feed is the process's, so a question of a parallel test
  moved it before this test's own question was listed, and the test took a question id from an empty list. It
  now waits for its broker's list first. A process has one broker outside tests.
- **Resolved by removal (2026-11-15, `remove-browser-access`).** Historical: **Fixed (2026-11-13): the `--open` smoke test read the launcher's file before it was written.** Seen once, in
  the release run of v0.4.0 (`explicit_serve_open_and_token_options_stay_gateway_local`,
  `crates/splitty-server/tests/http_surface.rs`): the test waited for the file the fake browser launcher appends
  the URL to and read it as soon as it existed; the shell's redirect creates the file before `printf` writes, so
  the test compared an empty text. It now waits for the complete line.
- **Fixed (2026-11-13): the keeper adoption test read the status record before it was applied.** Seen twice on
  2026-11-13 in full runs (`keeper::client::tests::released_keepers_are_adopted_with_their_output_and_process`,
  `splitty-core`, "the record rebuilt from the replay"): a chunk of output is in the scrollback before its status
  reports are applied (`Output::record_locked`, `crates/splitty-core/src/pty/session.rs`), and the test read the
  record as soon as the scrollback showed the replay. It now waits for the record.
- **Ending a native bridge can report a broken pipe** (seen 2026-11-12 in the release runs of v0.3.5 and v0.3.6;
  on two CPU cores in most runs, also at the commit that added the test, rarely on more): in
  `tests/version_skew.rs`, `the_clients_of_one_build_use_the_daemon_of_the_other` sends `detach` and finishes its
  `splitty_remote::bridge::NativeBridge`. Either the client's transport result or the `remote-bridge` process
  (exit status, stderr `Broken pipe (os error 32)`) then reports the closed pipe, with either build's bridge. The
  test accepts both. Not investigated: which side closes first, whether a client can lose its last frame that
  way, and whether the bridge should end successfully when its client is gone.
- **`splitty-client-core`'s native workspace tests fail now and then when the module runs in parallel** (seen
  2026-11-12 under WSL 2, at `47f7e95` and later; each passes alone): one to three of
  `native_workspace::close_tests::worker_maximize_filters_stale_visibility_and_size_reports_without_losing_output`
  (assertion at `native_workspace.rs:1978`, a window opened from another source),
  `close_tests::appearance_worker_failure_keeps_output_and_recovers_from_local_service`,
  `hub_tests::passive_directory_refresh_keeps_local_worker_and_cleanup_independent`,
  `hub_tests::pending_failed_hub_read_does_not_block_settings_or_terminal_output` and
  `ports_tests::ports_slow_start_keeps_terminal_host_writes_and_passive_reads_responsive` ("Ports worker deadline:
  Elapsed") fail in most runs of `cargo test -p splitty-client-core --lib native_workspace`, also with every
  inherited `SPLITTY_*` variable removed. **Mostly fixed (2026-11-13):** the crate's test services shared the
  process-wide stream admission of `splitty_transport::admission` (16 concurrent openings, 128 streams); a native
  workspace test alone holds up to 13 streams and 8 openings, so parallel tests exhausted it, and the rejected
  stream reported "transport byte budget exhausted" (admission reuses the byte budget's error). Every service and
  connector in the crate's unit tests now has its own admission and byte budget (`Connector::isolated()`);
  production connectors keep the process-wide limits. No permit outlived its test. Still seen afterwards, in 3 of
  10 full runs: `worker_maximize_filters_stale_visibility_and_size_reports_without_losing_output` (same assertion)
  and once `pending_failed_hub_read_does_not_block_settings_or_terminal_output` (deadline); not investigated.
  Seen again on 2026-11-14 in the static release build: both in one of 12 runs of the crate's unit tests, and
  `workspace::catalog::tests::service::maximized_projection_withdraws_only_hidden_size_claim_and_restores_measured_size`
  once in three workspace runs (no event within the 10 s of `Window::next`). Not investigated; one candidate is
  that the maximize tests let two clients claim a terminal's size without fixing the order in which the service
  reads them.
- **Fixed (2026-11-12): `splitty-remote`'s SSH option tests depended on the environment.** `askpass_program()` in
  `crates/splitty-remote/src/ssh.rs` falls back to `SPLITTY_ASKPASS_PROGRAM`, which the desktop app sets for the
  processes it starts, its terminal panes included; run from such a pane, four tests saw `BatchMode=no` and failed.
  The crate's tests no longer read the variable.
- **Fixed (2026-11-06): fixture ports and the instance lock in `splitty-server`'s tests.** Seen since 2026-10-31
  in full runs under load, and in the release run of v0.2.1
  (`hub::forwards::tests::stop_during_rebuild_waits_for_setup_and_port_release`); each passed alone. The tests
  of `http`, `hub::forwards` and `hub::reverse` had fixed ports (41001–41033) or ports the kernel handed out for
  port 0, and a port one test had just freed went to a parallel test before the first bound it again. They now
  take ports from one pool in `hub::testing::free_port` (from 20000 on, outside the kernel's automatic range,
  each port once), `host::relay`'s tests from a pool of their own (from 30000 on). Separately,
  `native::endpoint::tests::private_modes_both_peer_checks_and_instance_lock_are_mandatory` took the instance
  lock again right after dropping it, while a descriptor copy from a parallel test's fork could still hold the
  `flock` until its `exec`; it now waits up to 5 s for the lock, as the inheritance test already did. 20 runs of
  the full `splitty-server` unit tests passed in a row afterwards (before: about 1 failure in 8). The
  `splitty-remote` test listed here before, `forwards_take_the_asked_port_or_any_free_one`, no longer exists;
  the tunnel tests hold their port reservations.
- **Fixed (2026-11-14): the static server could crash when two terminals started at once.** Seen since v0.1.29
  as a SIGSEGV of the `splitty-core` unit tests for `x86_64-unknown-linux-musl` in release mode (the payload job
  of the release workflow), in about 1 of 15 runs, never with the glibc target; the release runs of v0.1.30 and
  v0.4.0 needed reruns. `portable-pty` looks up the user's shell and home directory with `getpwuid` whenever it
  builds or starts a command, whatever the environment says. `getpwuid` answers from one buffer of the C
  library, and musl grows that buffer without a lock, so two lookups at once corrupt it. The servers for WSL and
  for hosts are the same static build: two terminals starting at the same moment (a template with several panes,
  a restore) could end the server. One lock in `crates/splitty-core/src/pty/terminal.rs` now covers every
  command built or started; the unit test `commands_are_built_from_many_threads_at_once` ends the test process in
  the static build without it.
- Seen on 2026-11-07 in CI on `main`, each passing on the next commit without a related change:
  `daemon_lifecycle::remote_bridge_relays_native_bytes_and_detaches_without_ending_host_sessions` in the
  macOS job (`status.success()` at its bridge process), and
  `http::tests::a_forward_whose_ssh_ends_is_rebuilt_on_its_port` in "Rust tests". Locally,
  `remote_connect::the_hub_forwards_a_port_a_host_pane_listens_on` failed once in a full workspace run and passed
  alone. Not yet investigated.
- Seen on 2026-11-12 in CI on `main` for the v0.3.0 version commit, both passing in a rerun of the same commit:
  `native_workspace::close_tests::appearance_worker_failure_keeps_output_and_recovers_from_local_service` in
  "Rust tests" (`splitty-client-core`; no update within the 10 s of `next_update`), and
  `pty::session::tests::managed_input_backpressure_does_not_block_revocation_or_stop` in the macOS job
  (`splitty-core`, an assertion after the write loop that must see backpressure within 1 s). Both depend on
  wall-clock limits on a loaded runner. Not yet investigated.
