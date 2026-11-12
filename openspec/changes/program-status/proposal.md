# Proposal

Roadmap row 125, the third of three changes split from one package with `replay-terminal-modes` and `version-skew-smoke-test`, and taken out of `terminal-intelligence` Level 2. Related: `terminal-bell` and `native-notification-badges` (the same delivery), `mcp-server` (agents reading pane state), `plugin-system` (a source of status for programs that do not report).

## Why

Splitty knows what runs in a pane only as busy or not: the foreground process is the base shell or it is not. An agent that waits for a tool approval looks the same as one that works, and a user with several agent panes, some on hosts, has to look into each to find the one that needs them. Programs can report this themselves through the Program Status Protocol (OSC 7501, https://www.superlogical.com/rex/docs/build/program-status, revision 0.4 of 2026-10-10). The agents most often run in panes implement it already (checked on 2026-11-12): Claude Code 2.1.322 reports a root record and one record per subagent, but only once the terminal has answered its probe; Codex 0.167.2 sends reports and clears them before it suspends or hands the terminal to another program. The server already taps each session's output for titles and OSC 133, also while no client is connected, so it can be the terminal that protocol addresses, for every client at once and with no integration per program.

## What Changes

- **Reading reports**: the server's scanner reads OSC 7501 reports into records per session: a root record and child records by `id`, each with `state` (`idle`, `working`, `done`, `blocked`, `error`), the `kind` of a block (`permission`, `question`, `auth`), `progress`, `app`, `title` and `msg`. It applies the protocol's rules: a report replaces its record whole, and `clear` removes a record and the records beneath it, all records without an `id`. Pairs it cannot read are skipped and unknown keys ignored; a report that breaks a limit, whose base64 does not decode, whose text holds a control character or whose `state` is unknown is dropped whole. `working`, `blocked` and `idle` end at the next OSC 133 `A` or when the program exits, in a shell when the foreground process is the base shell again (the busy state the server already tracks); `done` and `error` survive both. A full reset removes every record, a soft reset or a switch between screens none. At the limit on records the least recently updated one goes.
- **Unknown sequences**: the scanner reads an OSC it does not interpret to its end. Today it stops at the number, so the BEL that ends such a sequence counts as a bell (see terminal-sessions in the findings), and Claude Code ends its OSC sequences with BEL.
- **Answering the probe**: the server answers `OSC 7501 ; ?` on the session's input, once per probe in live output, and never for a probe in output it reads again (a daemon that adopts a keeper's buffer). The answer is queued on the input before the output that carried the probe reaches any client, between two of a client's writes and never inside one. Programs send primary device attributes (`CSI c`) right after the probe and take an earlier answer to it as no support; Claude Code does, waits up to 2 seconds and, without an answer, reports nothing for the rest of its run. A session is then reported with no client connected, and several clients of one session do not each answer.
- **The probe stays out of the stream** (PS-1): the server removes the probe from the output it records and forwards, so neither a client's emulator nor, under `splitty attach`, the user's own terminal can answer it a second time; a second answer would reach the program as typed text. The replay filters (for `splitty attach` and in the web client) drop the probe as well, for output recorded before this change.
- **On the wire**: a session's records are sent with `created` and `attached` and as an event when they change, the way the OSC 133 state is, behind a capability. Older clients ignore the event. How records reach a client for sessions it does not stream is for the design (see below).
- **Notifications**: a root record that becomes `blocked`, `done` or `error` in a pane the user is not watching raises a notification through the existing path, with the record's message as text and a limit on how often one pane may notify. Settings get a switch like the other notification types. While a pane holds a status record, its bells and, once `terminal-bell` maps them, its OSC notifications raise no notification of their own (PS-2): a program that reports its state sends those as well, and the record says more. `done` and `error` stay on the server until the program replaces or clears them, a full reset or the end of the session; a window drops its mark for them once the user has seen the pane, as it does for an unread notification (PS-3).
- **In the native window**: a pane's tab shows its program's state (working, blocked with its kind, done, failed), and screens and host groups show that a pane in them is blocked.
- **For the CLI**: `splitty list-panes` shows each pane's root record, so an agent that started other agents in panes can see which one waits.

Out of scope:
- Parsers for programs that do not report (`terminal-intelligence`, Level 2), and hooks or plugins installed into agents (`plugin-system`).
- Acting on a status report: answering a permission request, typing into the pane.
- Mapping other sequences to status records: OSC 9;4 progress, the OSC 21337 tab status that Codex also writes, and the OSC 9/99/777 notifications (see `terminal-bell`).
- Status marks in the web shells, unless the design decides otherwise.

## Capabilities

### New Capabilities

- `program-status`: reading reports, the records and their lifetime, the probe's answer and its removal from the output, the event and the state on attach.

### Modified Capabilities

- `terminal-sessions`: "Continuous Output Capture" (status is tracked while no connection streams the session) and the server writing to a session's input on its own for the probe's answer.
- `mux-protocol` or `client-connections`: the capability and the status event.
- `cli`: the record in `list-panes`.
- `settings`: the notification switch.

## Impact

- Server: the OSC scanner (`pty/osc.rs`), which also learns to skip an OSC it does not interpret; the session's output record, state and watch channels (`pty/session.rs`); the probe's removal and answer on the session's output and input paths; the replay filters (`pty/replay.rs`, `frontend/src/lib/terminal/replay_output.ts`); protocol events and `docs/protocol.ts`; the notification engine (`protocol/notify.rs`); `list-panes`.
- Native client: the notification ledger of `splitty-client-core` (status notifications, and a pane's bells left out while it holds a record), tab and drawer marks in `splitty-gpui`.
- Hosts: status records come from the host's daemon; nothing new crosses the connection but the event.
- Tests: unit tests of the report parser and of a BEL that ends an unknown OSC; an in-process test with a real PTY where a program writes the probe and `CSI c` and reads the server's answer first, with no answer for a probe in output read again; the dispatcher's event and attach state; the ledger's notifications and marks.
- Security: `msg` and `title` are untrusted text from a program. They are shown as plain text, without control characters, bidi overrides or invisible formatting, and shortened. Anything shown from a record names its pane, and on a host the host, so a program cannot pose as one in another pane. The only bytes written back to a program are the probe's fixed answer. Notifications are rate-limited.

## Provisional decisions

Taken on 2026-11-12, open to the maintainer's review and listed in `openspec/open-decisions.md`; the design carries them on.

- **PS-1** (provisional, tbd): the server removes the status probe from the output it records and forwards. The alternative forwards it and relies on clients and the terminal under `splitty attach` not answering.
- **PS-2** (provisional, tbd): while a pane holds a status record, its bells and OSC notifications raise no notification of their own. The alternatives notify for both, or merge the two within a short time.
- **PS-3** (provisional, tbd): `done` and `error` records stay on the server until the program replaces or clears them, a full reset or the end of the session, and a window's mark for them goes once the pane is seen. The alternative lets the server drop them after a time or at the next input.

## Carried-over design notes

- (2026-11-12, review of the package draft) Claude Code 2.1.322 sends the status probe at startup together with its other terminal queries (XTVERSION, the kitty keyboard query, a DECRQM of `?2026`) and ends the batch with `CSI c`, waiting up to 2 seconds. It ends OSC sequences with BEL, or ST when it takes the terminal for kitty, reports one record per subagent besides the root and clears its records when it exits.
- (2026-11-12, review of the package draft) Codex 0.167.2 writes `OSC 7501 ; state=…` reports and `state=clear` before it suspends or runs an external program; no probe was found in its build. It also writes OSC 21337 (`status`, `indicator`, `status-color`, `detail`).
- (2026-11-12, review of the package draft) The notification engine tracks a session only while a connection streams it (terminal-sessions in the findings). Marks on screens and host groups for panes nobody streams need records outside the streams.

## Open questions for the design

- How records reach a client for sessions it does not stream (panes on screens not shown, host groups not opened), and with them notifications for such sessions: with the workspace's session state, or a status subscription per connection.
- How a program's other output changes once its probe is answered (a program may stop drawing its status into the title), and what Codex does with an answer. To be checked against installed versions.
- How child records (subagents) are shown in the first version, if at all, and whether `done` notifies or only `blocked` and `error`.
- The status protocol is young (four revisions in two weeks) and may change; which revision the first version follows.
- Whether a status record can also be set through the control API, for a local tool that learns a program's state out of band, and which source wins when both report. Reading agents' own data stays outside the server either way.
