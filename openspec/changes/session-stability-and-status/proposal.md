# Proposal

> Draft created on 2026-11-12. Specs, design and tasks are written when this change is next. It is the first of three packages: `server-terminal-state` follows and replaces its first part, `plugin-system` is the third. Its four parts are independent of each other and can become changes of their own. Related: `terminal-intelligence` (Level 2 keeps the app parsers for programs that do not report), `terminal-bell` and `native-notification-badges` (the same delivery), `mcp-server` (agents reading pane state).

## Why

Four gaps in what Splitty has today, none of which needs a new architecture.

**Terminal modes are lost in a replay.** A session's replay is the newest bytes of its output, 1 MiB by default. A mode a program set before the oldest retained byte is not in it, so a client that attaches with a fresh emulator never learns it: bracketed paste, the kitty keyboard flags, focus and mouse reporting, application cursor keys, the alternate screen. Programs set these once at startup, and a program that redraws often fills the buffer within minutes. The redraw after an attach repaints the screen and sets no mode again. Input would then behave differently after a reconnect or in a new window: a multi-line paste arrives as separate lines, modified keys lose their encoding, mouse input no longer reaches the program. This follows from the code and has not been reproduced yet; the first task is a test that shows it.

**A program's state is unknown.** Splitty knows what runs in a pane only as busy or not: the foreground process is the base shell or it is not. An agent that waits for a tool approval looks the same as one that works, and a user with several agent panes, some on hosts, has to look into each to find the one that needs them. Programs can report this themselves through the Program Status Protocol (OSC 7501, https://www.superlogical.com/rex/docs/build/program-status, revision 0.4 of 2026-10-10). The server already taps each session's output for titles and OSC 133, also while no client is connected, so it can be the terminal that protocol addresses, for every client at once and with no integration per program.

**No test runs two versions against each other.** Clients, host daemons, keepers and the `splitty` command in a pane change version independently, and each fallback is tested only against a simulated peer (see "version skew" in `docs/issues/openspec-baseline-findings.md`).

**On a Linux host, everything shares one control group.** The daemon, the keepers and every program started from one login are in the same control group. Memory pressure is resolved per group, and a service such as systemd-oomd ends a whole group, so one runaway program can end every session and the daemon with it. `setsid`, which the daemon and the keepers use, does not change the group.

## What Changes

### 1. Terminal modes restored on attach

- **Mode tracking**: the session's output scan also follows the modes that outlive the output that set them, with the output offset of each one's last change: the alternate screen (`?1049`, `?1047`, `?47`), bracketed paste (`?2004`), focus reporting (`?1004`), mouse tracking (`?9`, `?1000`, `?1002`, `?1003`) and its encodings (`?1005`, `?1006`, `?1015`, `?1016`), application cursor keys (`?1`) and keypad (`ESC =`, `ESC >`), cursor visibility (`?25`) and style (`DECSCUSR`), the kitty keyboard flags including their stack, and `modifyOtherKeys`. A full reset (`RIS`) clears them, a soft reset (`DECSTR`) the ones it resets in a terminal.
- **Restored on attach**: before the buffered output, the server sends the sequences that set every tracked mode whose last change lies before the oldest retained byte. Modes changed inside the retained output are set by the replay itself. The sequences travel as ordinary output frames of the replay, so no client changes: they are not report requests, and the replay filters keep them. `splitty attach` gets the same bytes through its session stream.
- Decided so far: a mode is restored ahead of the buffered output, by offset, so that output is interpreted in the state it was written in (setting all modes after the replay would paint alternate-screen output onto the normal screen first); and the tracker keeps a fixed set of flags and no screen, which leaves terminal state on the server to `server-terminal-state`.

### 2. Program status

- **Reading reports**: the server's scanner reads OSC 7501 reports into records per session: a root record and child records by `id`, each with `state` (`idle`, `working`, `done`, `blocked`, `error`), the `kind` of a block (`permission`, `question`, `auth`), `progress`, `app`, `title` and `msg`. It applies the protocol's rules: a report replaces its record, `clear` removes a record and its children, a full reset removes all, `working` and `blocked` end at the next OSC 133 `A` or when the program exits, malformed or oversized reports are dropped whole, and the limits on sizes and record counts hold.
- **Answering the probe**: the server answers `OSC 7501 ; ?` on the session's input, once per probe in live output, and never for a probe in output it reads again (a daemon that adopts a keeper's buffer). A session is then reported with no client connected, and several clients of one session do not each answer.
- **On the wire**: a session's records are sent with `created` and `attached` and as an event when they change, the way the OSC 133 state is, behind a capability. Older clients ignore the event.
- **Notifications**: a root record that becomes `blocked`, `done` or `error` in a pane the user is not watching raises a notification through the existing path, with the record's message as text and a limit on how often one pane may notify. Settings get a switch like the other notification types.
- **In the native window**: a pane's tab shows its program's state (working, blocked with its kind, done, failed), and screens and host groups show that a pane in them is blocked.
- **For the CLI**: `splitty list-panes` shows each pane's root record, so an agent that started other agents in panes can see which one waits.

### 3. A test across versions

- A smoke test starts the last released `splitty` next to the current build and creates, attaches, resizes and re-attaches a session across each boundary, in both directions: a client and a host's daemon; a daemon and the keepers an earlier version started; the `splitty` command in a pane and the daemon behind its `SPLITTY_SOCK`; the files one build writes and the other reads.
- It runs before a release. It is a smoke test because an in-process test links one version of every type.

### 4. A control group per session on Linux (to decide)

- On a Linux host with systemd, the daemon places itself and each keeper with its program in a scope of its own, so an out-of-memory kill is confined to one session. A memory limit per session is a possible setting on top and not part of a first version.
- A precondition to settle first: scopes belong to the user's service manager, which stops at the user's last logout unless lingering is enabled, and takes its scopes with it. Today a host's sessions survive a logout inside the login's own scope. Scopes are therefore used only where the user's service manager outlives logins; elsewhere placement stays as it is, and the daemon reports which case holds.
- Not on macOS, and under WSL only with systemd enabled. Hazards to design against: races with short-lived sessions, and scopes left behind.

Out of scope:
- Screen contents, scroll regions, tab stops, character sets and colors set before the retained output, the first retained byte lying inside an escape sequence, and a replay sized in lines (`server-terminal-state`).
- Parsers for programs that do not report (`terminal-intelligence`, Level 2), and hooks or plugins installed into agents (`plugin-system`).
- Acting on a status report: answering a permission request, typing into the pane.
- Mapping other sequences to status records: OSC 9;4 progress, and the OSC 9/99/777 notifications (see `terminal-bell`).

## Capabilities

To be decided when the change is next. Likely:

### New Capabilities
- `program-status`: reading reports, the records and their lifetime, the probe's answer, the event and the state on attach.

### Modified Capabilities
- `terminal-sessions`: "Attaching to an Existing Session" (the restored modes before the buffered output), "Continuous Output Capture" (modes and status are tracked while no connection streams the session), and the server writing to a session's input on its own for the probe's answer.
- `mux-protocol` or `client-connections`: the capability and the status event.
- `cli`: the record in `list-panes`.
- `settings`: the notification switch.
- `session-keepers` or `server-modes`: the placement of daemon and keepers in control groups, if part 4 goes ahead.

## Impact

- Server: a CSI and `ESC` tap next to the OSC scanner (`pty/osc.rs` reads OSC only), the session's output record, state and watch channels (`pty/session.rs`), protocol events and `docs/protocol.ts`, the notification engine (`protocol/notify.rs`), `list-panes`; for part 4 the daemon's start and the keeper launcher.
- Native client: the notification ledger of `splitty-client-core`, tab and drawer marks in `splitty-gpui`. The web shells ignore the status records unless decided otherwise. Part 1 changes no client.
- Hosts: status records come from the host's daemon; nothing new crosses the connection but the event.
- Tests and CI: unit tests of the tracker and of the report parser; one in-process test with a real PTY in `pty/session.rs`, where a program sets modes, prints more than the buffer holds, and a new subscriber's replay starts with them; the smoke test of part 3 and the step that fetches a released build for it.
- Security: `msg` and `title` are untrusted text from a program. They are shown as plain text, without control characters, bidi overrides or invisible formatting, and shortened. The only bytes written back to a program are the probe's fixed answer. Notifications are rate-limited.

## Open questions for the design

- Where the mode tracker and the status records live in daemon mode. In the session layer they are rebuilt from the keeper's oldest buffered byte after a daemon restart, like the OSC state, so what is older than the keeper's buffer is unknown again. In the keeper they would survive, behind a keeper capability that keepers started by an earlier version lack.
- Whether the server removes the status probe from the output it forwards, so a client emulator that implements the protocol does not answer a second time.
- Which programs send status reports, whether they wait for the probe's answer, and how their other output changes once it is answered (a program may stop drawing its status into the title). To be checked against installed versions.
- How child records (subagents) are shown in the first version, if at all, and whether `done` notifies or only `blocked` and `error`.
- The status protocol is young and may change; which revision the first version follows.
- Whether a status record can also be set through the control API, for a local tool that learns a program's state out of band, and which source wins when both report. Reading agents' own data stays outside the server either way.
- Part 3: where the released build comes from (by version and checksum), which boundaries the first version covers, and on which runs it is required.
- Part 4: how lingering is detected, how scopes are requested, and what the daemon does on a host without a user service manager.
