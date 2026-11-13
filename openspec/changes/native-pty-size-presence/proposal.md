# Proposal

## Why

The terminal-sessions spec requires clients of a server that announces `pty-size-v1` to tell it when their user acts and when their window is not in use, and to show each session at the PTY size the server's policy chose (Client Activity, Client Activity Reports, Client Display of the PTY Size). The web client does this; the native app does not:

- Its source worker sends `activity` with `takeover: true` after every `attached` event, also after a reconnect or when a window nobody touched opens a Screen, so such a window takes the size from a client in use.
- It sends no `activity` for the user's own interaction and no `idle` when the window is hidden, and nothing when a `mux.welcome` arrives.
- It ignores the `size` event: the terminal grid always follows the pane, so a session another client sized differently is drawn at the wrong width, and nothing says that the size is set elsewhere or offers to take it over.

## What Changes

- The source worker (client-core, for the local source and every host source alike) keeps per connection whether its server decides sizes, and what it last told it: on each `mux.welcome` with the capability it sends `activity` or `idle` before it attaches any session, according to whether the window is visible, focused and showing a Screen of that connection. It no longer sends `activity` on `attached`.
- The window reports the user's interaction (a key or pointer button in the window, the window becoming active, a change of its size) and its visibility and focus to the worker. The worker sends `activity` only to the connection of the shown Screen, at most once per 5 seconds per connection, and `idle` to every connection when the window becomes hidden.
- The worker validates `size` events (1–1000 columns, 1–500 rows, a known policy) of the sessions it attached and passes each new size to the window; others are ignored.
- The native terminal keeps its emulator at the session's PTY size from the latest `size` event (the pane's own size until one arrives) and reports the pane's size as before. Where the PTY is narrower or shorter than the pane, the remaining area is marked as set by another client; a taller session moves up so its cursor row stays visible. A badge with the PTY size takes the size over (`activity` with `takeover: true`) unless the policy is `smallest`.
- With a server that does not announce the capability, the worker sends no `activity`, `idle` or `hide`, and its first report after an attach resizes the PTY by one row and back instead of asking for `redraw`.

## Capabilities

### New Capabilities

### Modified Capabilities

None: the requirements exist in `terminal-sessions`; this change implements them in the native app.

## Impact

- `crates/splitty-client-core/src/native_workspace/presence.rs` (new: the window's presence and interaction cell, per-connection reports, `size` validation), `native_workspace/source.rs` (welcome greeting, size events, capability gating), `native_workspace/remote.rs` (presence for host sources), `native_workspace.rs` (the cell read at each turn, command `TakeOverSize`, update `PtySize`).
- `crates/splitty-gpui/src/terminal.rs` (emulator at the PTY size, letterbox, cursor-row shift, takeover badge), `terminal_engine.rs` (cursor row of the active area), `workspace.rs` (interaction and presence reports, size updates to the views).
- Protocol unchanged; the messages exist.

## Architecture impact

No new component or relationship; the descriptions of `native_workspace -> splitty.core.protocol` and `gpui_workspace -> splitty.client_core.native_workspace` mention the size capability and the presence reports, and `docs/architecture.md` describes the native size display beside the size reports.

## Decisions

- The window writes its visibility, focus and the fact that its user acted into a cell it shares with its worker instead of sending commands: typing adds nothing to the worker's bounded queue, and a welcome is greeted with the presence of that moment. The worker reads the cell at each turn (at least every 16 ms), routes an interaction to the connection of the Screen shown then and applies the protocol's limit of one `activity` per 5 seconds per connection.
- The emulator takes a new size only with the server's `size` answer, as in the web client: output written before the PTY took the size is parsed at the size it was written for.
- NPS-1 (provisional, tbd): interaction is a key or pointer button anywhere in the window, the window becoming the active window, and a change of the window's size; scrolling and pointer movement are not. The same set as the web client.
- NPS-2 (provisional, tbd): "hidden" is the platform no longer presenting the window's frames (minimized, fully covered, display asleep); only then is `idle` sent. Losing focus alone sends nothing, and becoming visible again sends nothing until the user acts.
- NPS-3 (provisional, tbd): the area outside a smaller PTY grid is hatched with thin diagonal lines in the palette's dim color with a dashed edge, and a badge in the pane's top right corner shows the size (`100×30`) in the look of the scroll-to-bottom control. Its tooltip reads "Another client set this terminal to 100×30. Click to use this window's size." or, under `smallest`, "Another client shows this terminal at 100×30." and the badge then takes no click. The same wording as the web client.
