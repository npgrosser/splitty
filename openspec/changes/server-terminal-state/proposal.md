# Proposal

> Draft created on 2026-11-12. Not planned yet. It is the second of three packages, after `session-stability-and-status` (whose restored terminal modes this change replaces) and before `plugin-system`. Specs, design and tasks are written when this change is next. Related: `terminal-intelligence` (its design leaves a server-side emulator under review; this draft is that review's proposal), `mcp-server` (agents reading a pane's screen), `multi-attach`.

## Why

A client that attaches to a session gets the newest bytes of the session's output and interprets them from nothing. That is right while the buffer holds everything that shaped the terminal, and wrong without a sign once it does not:

- Modes, screen contents and history set before the oldest retained byte are gone (`session-stability-and-status` restores the modes).
- A program that repaints fills the buffer with repaints, so the retained output covers minutes of work instead of the history a user scrolls back to.
- Historical output has effects when it is interpreted again: report requests, clipboard writes, notifications. Every client filters them by a list that each new sequence with an effect has to join, and the findings document lists the faults this has produced.
- A full-screen program is correct only after the redraw a client forces once it is attached.
- Nothing on the server knows what a pane shows. The CLI cannot print a pane's screen, an agent cannot read another pane, and job logs are escape sequences stripped from a byte stream instead of text read off a screen.

The server owns sessions so that clients can come and go. What a session's terminal looks like is part of the session.

## What Changes

- **A terminal per session on the server**: the session layer feeds each session's output into a terminal emulator of the PTY's size, next to the scanner, whether or not a client is connected. The server resizes the PTY and the emulator in one step, so the two never differ.
- **The replay becomes a snapshot**: on `attach` the server sends the session's `size`, then the terminal's state written as terminal output: history, the screen (both screens while the alternate one is active), then modes, scroll region, tab stops, character sets, colors the program set, title and working directory, and the cursor last. Live output follows from the exact output position the snapshot was taken at. A snapshot contains no report request and nothing with an effect outside the terminal.
- **Clients keep their emulators**: the snapshot travels in the replay's output frames, as bytes any terminal understands. Each client interprets it with the emulator it has: libghostty-vt in the native window, xterm.js or ghostty-web in the web shells, the user's own terminal under `splitty attach`. No client depends on the emulator the server uses, and the server's emulator can be replaced without a protocol change. A client of an earlier version works unchanged.
- **A capability**: behind it a client stops filtering the replay and stops requesting the redraw after an attach. Against a server without it, clients behave as today.
- **History by content**: what is retained is what scrolled off the terminal, not the last bytes written. Its budget stays a setting.
- **Daemon restarts**: keepers keep their byte buffer and their protocol. A daemon that adopts a keeper rebuilds the session's terminal from the keeper's buffer, the way it rebuilds the OSC state today, so after a restart the state is as complete as that buffer.

- **A pane's screen as text**: the control API and the CLI (`splitty capture-pane`, the counterpart of `send-keys`; name provisional) return what a pane shows: the visible screen and, on request, its history, as plain text or with its styling as escape sequences, together with the terminal's size and cursor position. Reading takes no part in the session: it attaches nothing, does not count for the PTY's size and changes nothing. The owner reads any pane; a restricted agent reads only the panes of its own jobs.

Follow-ups this enables, each a change of its own: the same read as a tool for agents (`mcp-server`), app parsers that match the screen (`terminal-intelligence`, Level 2), previews of panes no window has attached.

Out of scope:
- Sending cells or screen differences to clients. Clients stay terminals that are fed bytes.
- A terminal emulator in the keeper.
- Changing which emulator a client uses.

## Capabilities

To be decided when the change is next. Likely:

### Modified Capabilities
- `terminal-sessions`: "Scrollback Buffer" (history by content), "Attaching to an Existing Session" (a snapshot instead of buffered bytes; "exactly once" then holds from the snapshot's position), "Continuous Output Capture", "Client Handling of Session Streams" and the redraw of the first size report (neither filtering nor redraw behind the capability).
- `session-keepers`: adoption rebuilds the terminal's state from the keeper's buffer.
- `mux-protocol` or `client-connections`: the capability.
- `settings`: the history budget.
- `control-api` and `cli`: reading a pane's screen and history; `access-control` for who may read which pane.

## Impact

- Server: the session layer (`pty/session.rs`), the attach path of the dispatcher, sizing, and a terminal engine in `splitty-core` or a crate of its own (a new component in the architecture model, shared with `splitty-gpui`'s `terminal_engine` if both use the same library).
- Build: the server links a terminal emulator for every host target (Linux x86_64 and aarch64 with musl, macOS on both architectures). Today only `splitty-gpui` links one, for the desktop targets, and the workspace's tests run without it.
- Clients: the native window, the web shells and `splitty attach` drop their replay filter and the redraw behind the capability and keep both for older servers and hosts.
- Hosts: every session parses its output at all times and holds its history as cells, which costs more memory and CPU than a byte buffer.
- Security: a host's daemon then interprets terminal output, which so far only clients did. In daemon mode a fault in the emulator ends the daemon and no session, since keepers hold them; the desktop's own backend runs in a process that already uses an emulator for display.
- Docs: `docs/architecture.md` (attaching, PTY size, keepers), `docs/protocol.ts`.

## Open questions for the design

- **Which emulator.** libghostty-vt is what the native window displays with, so the server and the main client would read output the same way, and its formatter writes a terminal's state as terminal output (upstream documents modes, keyboard flags, scroll region, tab stops, character sets, palette and cursor; to be confirmed for the pinned version). To check there: formatting the screen that is not active, which the history of the normal screen needs while a program uses the alternate one; images; OSC 133 marks; hyperlinks. The alternative is an emulator written in Rust, with no foreign build for host targets and tests that need no bootstrap, and a serializer of our own. Either sits behind one internal interface: feed, resize, snapshot, screen text.
- **Where emulators disagree.** On character widths, grapheme clusters or the reflow after a resize, a snapshot shows the server's reading until the program draws again. How a snapshot is written to keep such a difference inside one row (rows placed by position, no reliance on wrapping).
- **Size and order of a snapshot.** History first delays the screen. Whether the screen goes first, or history only on request.
- **Budget.** History per session in lines or bytes, its default, and whether sessions without a pane and managed jobs get a terminal.
- **After a daemon restart.** Whether state rebuilt from the keeper's buffer is enough, or the daemon hands the keeper a snapshot to keep.
- **The byte buffer.** Whether the session layer keeps one for log cursors and for a hub that relays a host without the capability.
- **Images.** Whether kitty graphics and sixel output survive a snapshot, which a byte replay gives while they are still in the buffer.
- **Job logs.** Whether a job's text log is read from the terminal's lines instead of escape sequences stripped from the byte stream, and what its cursors then count.
- **Reading a screen.** What the read returns for a pane on a host, how much history one request may return, and whether a caller can wait until the screen shows a given text.
