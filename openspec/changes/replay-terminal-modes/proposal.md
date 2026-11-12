# Proposal

## Why

A session's replay is the newest bytes of its output, 1 MiB by default. A mode a program set before the oldest retained byte is not in it, so a client that attaches with a fresh emulator never learns it: bracketed paste, the kitty keyboard flags, focus and mouse reporting, application cursor keys, the alternate screen. Programs set these once at startup (Claude Code 2.1.322 resets its modes only when it exits), a program that redraws often fills the buffer within minutes, and the redraw after an attach repaints the screen and sets no mode again. After a reconnect or in a new window a multi-line paste then arrives as separate lines, modified keys lose their encoding and mouse input no longer reaches the program. This follows from the code and has not been reproduced yet; the first task is a test that shows it.

Roadmap row 125, the first of three changes split from one package with `version-skew-smoke-test` and `program-status`. `server-terminal-state` later replaces it with a snapshot of the whole terminal.

## What Changes

- **Mode tracking**: the session's output scan also follows the modes that outlive the output that set them, with the output offset of each change: the alternate screen (`?1049`, `?1047`, `?47`), autowrap (`?7`), bracketed paste (`?2004`), focus reporting (`?1004`), mouse tracking (`?9`, `?1000`, `?1002`, `?1003`) and its encodings (`?1005`, `?1006`, `?1015`, `?1016`), alternate scroll (`?1007`), application cursor keys (`?1`) and keypad (`ESC =`, `ESC >`), cursor visibility (`?25`) and style (`DECSCUSR`), the kitty keyboard flags with their stack, which the kitty protocol keeps per screen, `modifyOtherKeys`, color-scheme reports (`?2031`) and in-band size reports (`?2048`). Synchronized output (`?2026`) belongs to one frame and is never restored. A full reset (`RIS`) clears the tracked modes, a soft reset (`DECSTR`) the ones it resets in a terminal. Modes are tracked whether or not a client is connected.
- **Restored on attach**: before the buffered output, the server sends the sequences that put the terminal into the state the retained output starts in: the alternate screen as it was at the oldest retained byte, and every other tracked mode as its last change before that byte left it. Modes changed inside the retained output are set by the replay itself. A mode set before the oldest byte and changed inside the retained output is at its default until that change: for input modes only the state after the replay counts, and the replay sets it; for the alternate screen, where the output in between would land on the wrong screen, the tracker keeps the changes inside the retained output, so the state before the first one is known. The kitty flags are restored as the main screen's stack, the screen switch, then the alternate screen's stack.
- **No client changes**: the sequences travel as ordinary output frames of the replay. They are not report requests, so the replay filters keep them, and `splitty attach` gets the same bytes through its session stream.

Out of scope:
- Screen contents, scroll regions, tab stops, character sets and colors set before the retained output, the first retained byte lying inside an escape sequence, and a replay sized in lines (`server-terminal-state`).
- Modes after a daemon restart that are older than the keeper's buffer (see RTM-1).
- Several clients answering the reports a restored mode asks for (focus, color scheme, size): that holds for modes set live today as well.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `terminal-sessions`: "Attaching to an Existing Session" (the restored modes ahead of the buffered output) and "Continuous Output Capture" (the modes are tracked while no connection streams the session).

## Impact

- Server: a CSI and `ESC` tap next to the OSC scanner in the session's output scan (`pty/osc.rs` reads OSC only, `pty/session.rs` records each chunk), and the snapshot a subscriber takes in the same lock as the chunks it does not get.
- Clients: none. The web shells, the native window and `splitty attach` interpret the restored sequences like any other output.
- Tests: unit tests of the tracker; an in-process test with a real PTY in `pty/session.rs` where a program sets modes, prints more than the buffer holds, and a new subscriber's replay starts with them.
- Findings: "The replay starts wherever the ring cut" in `docs/issues/openspec-baseline-findings.md` narrows to what this change leaves out.

## Architecture impact

None in `architecture/*.c4`: the tracker is part of the session layer's output scan, inside the existing `pty` component, with no new dependency. The session output section of `docs/architecture.md` names it, if it describes the scan.

## Decisions

- A mode is restored ahead of the buffered output, by offset, so that output is interpreted in the state it was written in; setting all modes after the replay would paint alternate-screen output onto the normal screen first.
- The tracker keeps a fixed set of flags and no screen, which leaves terminal state on the server to `server-terminal-state`.
- **RTM-1** (2026-11-12, provisional, tbd): the tracker lives in the session layer. After a daemon restart it is rebuilt from the keeper's oldest buffered byte, like the OSC state, so a mode older than the keeper's buffer is lost again after each restart, such as a host's update. Attaches and reconnects happen far more often than restarts; a tracker in the keeper, behind a keeper capability that keepers started by an earlier version lack, is a later step.
- **RTM-2** (2026-11-12, provisional, tbd): the alternate screen is restored to its state at the oldest retained byte, every other mode to its last change before that byte; `?7`, `?1007`, `?2031` and `?2048` are tracked, `?2026` never. The alternatives are the state at the oldest byte for every mode, which needs the changes inside the retained output for every mode, or the last change for every mode, the alternate screen included.
