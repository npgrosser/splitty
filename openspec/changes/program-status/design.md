# Design

## Context

Every chunk of a session's output passes one record path in the session layer, under one output lock: the scrollback ring, the mode tracker, the job log text, the OSC scanner (titles, OSC 133, bells) and the subscribers, from the first byte on and whether or not a client streams the session. A daemon's sessions get their output from a session keeper; after a daemon restart the adopted session is fed again from the keeper's oldest buffered byte, so the scrollback and the OSC state come back.

The OSC scanner reads OSC 0, 2 and 133. For any other number it returns to its idle state at the `;`, so the BEL that ends the sequence counts as a bell; the same happens for a number of five digits (OSC 21337) and for a title or OSC 133 payload over its limit. It also takes the byte 0x9d as an OSC introducer, which in UTF-8 output is a continuation byte.

The notification engine of the protocol layer tracks a session only while a connection streams it, and its watchers run per stream. `ports.changed` is the precedent for state of all sessions: a watcher in the server's client sessions sends the full list right after each `mux.welcome` and to every client on change.

The native window has one connection per source: the local service and each host it has connected. A host's messages arrive on that host's connection; the hub relays them unchanged. The window streams every session of its local screens, but of a host only the screens it has opened; it knows the host's screen catalog either way.

The programs this is for were checked against their installed builds on 2026-11-12:

- **Claude Code 2.1.322** sends the probe at startup with its other terminal queries and ends the batch with `CSI c`. It takes any OSC 7501 reply whose payload begins with `?` as support, a `CSI c` reply that comes first as no support, and waits up to 2 seconds. Once supported, it reports a root record with `app=claude-code`: `working` with progress, `blocked` with a kind and a message (permission prompts, questions, authentication), `done` when a turn completes, `idle`, `error`. Child records stand for background agents (`working`, or `blocked` with `question` or `permission` for paused phases); permission prompts are on the root. It sends `state=clear` when it restores the terminal. Its title writes do not change once the probe is answered. It reports nothing when its terminal title is turned off (`CLAUDE_CODE_DISABLE_TERMINAL_TITLE`) and in its background workers. It writes OSC 133 marks only in its screen-reader mode.
- **Codex 0.167.2** sends no probe. It writes reports ended by `ESC \` and `state=clear` before it suspends or hands the terminal to another program, and OSC 21337 ended by BEL. An answer changes nothing for it.

## Goals / Non-Goals

**Goals:**

- The native window shows whether the agent in a pane works, waits for the user, is done or failed, for local panes and for panes of connected hosts, also on screens it has not opened, with one implementation in the server and none per program.
- A notification when an agent in a pane the user is not watching waits for them or is done.
- `splitty list-panes` and the control API show each pane's state, for agents that drive other panes.
- The BEL that ends an OSC no longer counts as a bell.

**Non-Goals:**

- Showing child records; setting records through the control API; marks in the web shells.
- Mapping OSC 21337, OSC 9;4 or the OSC 9/99/777 notifications to records; acting on a report; parsers for programs that do not report.
- Keeping records across a server restart other than by reading a keeper's buffer again.

## Decisions

### D1: The probe is removed first in the record path

A matcher for exactly `ESC ] 7501 ; ?` followed by BEL or `ESC \` runs on each chunk before anything else in the record path. It removes each probe and queues its answer; the scrollback, the mode tracker, the log text, the OSC scanner and the subscribers get the rest. At a chunk's end it holds back a beginning of the probe, at most the 9 bytes `ESC ] 7501 ; ? ESC`, and passes it on with the next chunk or at the end of the output. Held bytes are in neither a snapshot nor a receiver yet, so each byte still reaches a subscriber once.

The matcher does not know whether it is inside another string, so a probe inside a DCS or APC string is removed and answered too. No program is known to nest one.

- Alternative: remove the probe in the OSC scanner. The scanner runs after the scrollback is written, and the probe must be gone before any consumer sees the bytes.
- Alternative: a matcher that skips strings. More state for a case that does not occur.

### D2: Answers go through one input gate per session

Every write to a session's terminal goes through one entry point that holds an asynchronous gate for the whole write: client `input` and `input_bytes`, the control API's keys and the delayed startup command, which today writes to the terminal directly. Holding the gate, a writer first writes the queued answers and then its own bytes. A task woken when an answer is queued writes answers while nobody else writes. The record path queues an answer before it hands the chunk to subscribers, so a client that replies to the `CSI c` it received after the probe finds the answer ahead of its reply. A kept terminal writes large input in several frames; they all fall within one hold of the gate. At most 16 answers wait per session (PS-10); a failed answer is logged and dropped.

- Alternative: write the answer from the reader thread. It would block that thread on a full PTY and race with client writes.
- Alternative: a lock inside each terminal. Two implementations, and a kept terminal has no lock across its frames.

### D3: The scanner reads every OSC to its end

After `ESC ]` the scanner reads the number. For 0, 2, 133 and 7501 it collects the payload up to its limit; for any other number, and once a payload is over its limit, it skips to the end without collecting. An OSC ends at BEL or `ESC \`; CAN and SUB cancel it, and an ESC followed by anything but `\` cancels it and begins a new sequence. The BEL that ends an OSC is never a bell. 0x9d no longer begins an OSC (PS-11), as the mode tracker already ignores 8-bit CSI. `ESC c` outside a string is reported as a full reset. The whole OSC 7501 sequence is limited to 4096 bytes.

The scanner returns its findings for a chunk as one list in byte order (titles, OSC 133 actions, status reports, resets, bells), so the status records apply an OSC 133 `A` and a report in the order the program wrote them. Titles, OSC 133 state and bells keep their watch channels.

- Alternative: a second parser for OSC 7501 next to the scanner. Two parsers would disagree about where a string ends.
- Alternative: merge the scanner into the mode tracker, which already skips strings. A larger change than this one needs.

### D4: Records live in the session layer

A status store per session sits next to the scanner under the output lock. It parses reports (fields, limits, base64 with and without padding, sanitizing), keeps at most 64 records with the root never evicted (PS-10), and counts a revision that changes with the root record. It removes `working`, `blocked` and `idle` records at OSC 133 `A`, at the end of the output, and when told that the session stopped being busy; it removes all records at a full reset. Each root record carries the time of its report and whether it came from output read again (D5).

The pane-state observer of the service already samples every pane's busy state twice a second; on a change from busy to not busy it tells the session (PS-14).

- Alternative: keep the records in the workspace state. Every report would change the mux store and nudge clients, and records would be persisted with the workspace.
- Alternative: keep them in the server crate. Only the session layer sees the output in order.

### D5: Output a keeper had buffered is read again, without answers

The session layer gets a second entry point for output read again. A kept terminal sends the bytes below the keeper's `hello` `buffer_end`, in keeper offsets, through it and splits a frame that straddles that offset; this applies to an adoption and to a reconnect. Such output has its probes removed and rebuilds the records, but queues no answer, and its root changes are marked read again so they notify nobody (PS-13). `splitty attach` without a daemon already splits the keeper's output at the same offset for its replay filter; it forwards live probes to the user's terminal, which is then the only terminal.

- Alternative: adopted sessions start without records. Every daemon update would drop the marks of waiting agents.
- Alternative: answer probes in buffered output too. A program that probed before the restart would get a second answer as typed input, and a stale answer can reach a shell.

### D6: One event for every connection, from a watcher next to the ports

A program-status watcher in the server's client sessions works like the ports watcher. While clients are connected it checks each session's status revision every 100 ms and sends `program_status.changed` for every root record that changed, to every client through the notification queues (PS-4). Right after a client's `mux.welcome` it sends that client the root records it holds. It maps sessions to their pane and screen from the mux store, rebuilt when the workspace changes, and sends again for a session whose pane moved to another screen. The check costs one atomic read per session and paces events to one per session every 100 ms (PS-10). The notification engine is not used.

- Alternative: records in `created` and `attached` and a watcher per stream, as for OSC 133. Only streamed sessions would have them, and the window streams no session of a host screen it has not opened.
- Alternative: a status subscription per connection. A new request, and every window would subscribe to everything.

### D7: The server marks `notify`, the window applies the user's view

The watcher marks an event `notify` as the spec states: the root record is `blocked`, `done` or `error`, differs in state, kind or message from the last one sent, did not come from output read again, and the session's last marked event with that state is at least 10 seconds old (PS-7). It keeps the last sent record and the times of marked events per session. The greeting after `mux.welcome` and the event for a moved pane are never marked. The window adds a notification for a marked event of a pane the user is not watching, if its settings allow it (PS-6).

- Alternative: the window derives notifications from changes. Each window would need its own history, and a greeting after a reconnect would look like a change.

### D8: Wire shape

```ts
/** A session's root program status record (OSC 7501). */
interface ProgramStatusRecord {
  state: "idle" | "working" | "done" | "blocked" | "error";
  /** Only with "blocked". */
  kind?: "permission" | "question" | "auth";
  /** 0..100, only with "working" and "blocked". */
  progress?: number;
  app?: string;
  /** Decoded and sanitized plain text. */
  title?: string;
  msg?: string;
  /** Unix milliseconds of the report, server clock. */
  updatedAt: number;
}

/** "program-status-v1". */
interface ProgramStatusChanged extends SessionEvent {
  type: "program_status.changed";
  paneId?: string;
  screenId?: string;
  status: ProgramStatusRecord | null;
  notify?: true;
}
```

`SessionState` gets `programStatus?: ProgramStatusRecord`, carried only by `GET /api/mux/state` and `GET /api/mux/screen/{id}`; screen snapshots over the WebSocket leave it out, the inverse of `scratchpad` (PS-15). The probe's answer is `ESC ] 7501 ; ?` with the probe's terminator and no lists (PS-8).

### D9: The window keeps a status table per source

`splitty-client-core` keeps, per source workspace, the root records from that source's events, accepted only from its current connection once its `mux.welcome` listed `program-status-v1`, as `ports.changed` is accepted today. The table is bounded (at most 4096 sessions per source; texts within the protocol's limits) and is cleared when the connection is replaced, before the greeting fills it again. A record marks the pane bound to its session in that source, or the pane and screen the event names when the window has no snapshot of that screen; a screen id the source's catalog does not know marks nothing. The window keeps, in memory, the `updatedAt` of the last `done` or `error` record the user saw per session, so a greeting after a reconnect does not mark it again (PS-17).

The notification ledger gets a program-status kind, enabled by `notifications.programStatus`: a warning for `blocked`, a success for `done`, an error for `error`. The text is the state with its kind and the record's `msg` (else `title`) as shortened plain text, with the screen and the host prefix the ledger already adds. The ledger also skips bell and OSC notification kinds for a pane that holds a root record (PS-2, PS-12); the window adds no such kinds yet, so this only takes effect with `terminal-bell`. GPUI shows the state on the tab and a blocked mark on drawer screens and host groups.

- Alternative: let the gpui crate keep the table. The ledger and the drawer need it, and both read from client-core.

### D10: Control API, CLI and settings

`GET /api/mux/state` and `GET /api/mux/screen/{id}` add each session's root record from the backend. `list-panes` appends `status=<state>[:<kind>]` or `status=-`, and with `--json` copies `programStatus` from a pane's session to the pane. The setting is `notifications.programStatus`; it defaults to true for stored settings without it and for an older server's reply, is listed in the metadata, and gets a row in the native settings editor. The web shells do not show it.

- Alternative: a status endpoint of its own. The CLI already reads the screen.

### Open questions of the proposal

- Sessions not streamed, and their notifications: one event to every connection (D6, PS-4).
- A program's other output once answered: unchanged for Claude Code; Codex never probes (Context).
- Child records: kept on the server for `clear` and the limit, not sent (PS-5). `done` notifies (PS-6).
- Protocol revision: 0.4 (PS-8). Control API setter: none (PS-9).
- Wire shape (D8), switch name (D10), rate limits and record limits (PS-7, PS-10), where bell suppression happens (PS-12).

## Architecture impact

- No new crate, top-level module, component or relationship. The new code lies in modules of existing components: the probe matcher and the status store in `splitty_core::pty` (`pty`), the buffered-output entry point in `keeper`, the watcher in `splitty_server::client_session` (`client_session`, next to the ports watcher), the busy hand-off and API fields in `service`, the routing of the event in `native_workspace`, the status table in `splitty_client_core::workspace` (`client_workspace`), the ledger kind in `client_notifications`, and the marks in `gpui_workspace`. The references they add are covered by `keeper -> pty`, `client_session -> pty`, `client_session -> mux`, `client_session -> protocol`, `service -> pty`, `native_workspace -> client_workspace`, `client_notifications -> client_workspace`, `gpui_workspace -> client_workspace` and `gpui_workspace -> client_notifications`; if the check finds one missing, the relationship is added in this change.
- `architecture/core.c4`: the descriptions of `pty` (probe removal and answers, status records) and `keeper` (output buffered before the controller connected). `architecture/server.c4`: `client_session` (`program_status.changed` after each welcome and on change), `service` (busy changes end records; status in the mux state replies) and the title of `service -> splitty.core.pty`. `architecture/client-core.c4`: `client_workspace` and `client_notifications`. `architecture/gpui.c4`: the title of `gpui_workspace -> splitty.client_core.client_workspace`.
- `docs/architecture.md`: "Session Output" (probe removal and the input gate, the scanner reading OSC to its end, the status store, the watcher), "Server-derived Pane State" (busy changes end records), "Session Keepers" (adoption reads buffered output again without answers), the `NotificationCenter` paragraph (the program-status kind and the bell rule) and "Settings authority" (seven notification booleans).

## Risks / Trade-offs

- **Held-back bytes arrive later.** A beginning of the probe at a chunk's end waits for the next read. Such bytes are an unfinished escape sequence that a terminal would not act on either.
- **An answer can reach the wrong program.** A program that exits right after its probe leaves the answer to the shell, as with any terminal reply. The flush task writes at once, and buffered output is never answered.
- **A probe during a daemon restart gets no answer.** It is in buffered output; Claude Code then reports nothing for that run.
- **OSC 133 `A` ends a block.** The protocol drops `working` and `blocked` at a prompt mark; Claude Code in its screen-reader mode writes such marks.
- **An OSC that never ends hides bells and titles** until the next ESC, as in other terminals.
- **The busy check has a 500 ms window.** A program that starts and reports right after another ended can keep the earlier program's child records until the next prompt mark.
- **Event load.** At most 10 events per session per second go to every connection; a client that cannot keep up is closed like for any other event.
- **The protocol is young.** A later revision may need `program-status-v2`; the parser ignores unknown keys, so additive changes are read as before.
- **Older daemons drop settings they do not know** (`docs/issues/openspec-baseline-findings.md`, server-modes), so a switched-off `programStatus` can come back on after running an older daemon.

## Migration Plan

Nothing is stored. The server part works with older clients, which ignore the event and no longer see probes; then the native part. Rolling back leaves programs without an answer, and they report nothing, as before.

## Provisional decisions

Taken on 2026-11-12, open to the maintainer's review; PS-1 to PS-3 come from the proposal, the others from this design, and each also goes into `openspec/open-decisions.md`.

- **PS-1** (provisional, tbd): the server removes the status probe from the output it records and forwards. Alternative: forward it and rely on clients and the terminal under `splitty attach` not answering.
- **PS-2** (provisional, tbd): while a pane holds a status record, its bells and OSC notifications raise no notification of their own. Alternatives: notify for both, or merge the two within a short time.
- **PS-3** (provisional, tbd): `done` and `error` records stay on the server until the program replaces or clears them, a full reset or the end of the session, and a window's mark for them goes once the pane is seen. Alternative: the server drops them after a time or at the next input.
- **PS-4** (provisional, tbd): records reach clients as one event sent to every connection, with the session's pane and screen, the held records right after `mux.welcome`, not in `created` or `attached`. Alternative: fields in `created` and `attached`, or a status subscription per connection.
- **PS-5** (provisional, tbd): the first version sends and shows only the root record; child records are kept on the server for `clear` and the record limit. Alternative: send all records, or let a blocked child mark its pane.
- **PS-6** (provisional, tbd): `done` notifies like `blocked` and `error`, all under one switch `notifications.programStatus` that defaults to on. Alternative: `done` only marks, or one switch per state.
- **PS-7** (provisional, tbd): the server marks an event `notify` for a root record that turns `blocked`, `done` or `error` or changes kind or message, from live output, at most once per session and state in 10 seconds; the window applies watched panes and settings. Alternative: the window derives notifications from changes with its own limit.
- **PS-8** (provisional, tbd): the first version follows revision 0.4, and the answer lists no states or kinds and ends with the probe's terminator. Alternative: list the states and kinds, or always end with `ESC \`.
- **PS-9** (provisional, tbd): no control-API request sets a record in the first version. Alternative: `POST /api/session/{id}/status`, with the program's own reports winning.
- **PS-10** (provisional, tbd): at most 64 records per session with the root never evicted, one event per session every 100 ms, at most 16 waiting answers per session, a hold-back of at most 9 bytes. Alternative: the protocol's maximum of 256 records with the root evicted like any other.
- **PS-11** (provisional, tbd): the byte 0x9d no longer begins an OSC. Alternative: keep it for OSC 0, 2 and 133 and leave it out for 7501.
- **PS-12** (provisional, tbd): PS-2 is applied in the window's notification ledger. Alternative: the server marks the bell events of a session that holds a record.
- **PS-13** (provisional, tbd): output a keeper had buffered before the daemon connected rebuilds records, but gets no answers and marks no notifications, at adoption and reconnect alike. Alternative: adopted sessions start without records.
- **PS-14** (provisional, tbd): `working`, `blocked` and `idle` records end at OSC 133 `A`, when a pane's session stops being busy (sampled every 500 ms) and at the end of the output. Alternative: end them whenever the session is not busy, which drops records a shell sets itself, or only at OSC 133 `A` and the end of the output, which keeps a killed agent's block in shells without integration.
- **PS-15** (provisional, tbd): the root record is in the mux state replies of the control API and in `list-panes` (`status=<state>[:<kind>]`), not in the workspace state or screen snapshots. Alternative: keep it in the workspace state with a nudge per change, or a status endpoint of its own.
- **PS-16** (provisional, tbd): the server removes bidirectional controls, invisible characters and line and paragraph separators from `title` and `msg`; the window shows plain, shortened text. Alternative: drop reports that contain them.
- **PS-17** (provisional, tbd): tabs show `working`, `blocked` with its kind, `done` and failed; screens and host groups show only `blocked`; what the user saw is kept in memory per window by the record's time; a host that is not connected or older shows nothing. Alternative: screens and host groups show `done` and `error` too, or the window persists what the user saw.
