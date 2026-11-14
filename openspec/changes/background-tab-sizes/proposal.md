# Proposal

## Why

A client reports a terminal's size only for a shown pane and withdraws it when the pane stops being shown. A tab in the background of a tab group therefore keeps its old PTY size while the window is resized, and its program is resized and redraws only when the user switches to that tab.

## What Changes

- The native app reports the other terminal tabs of a shown pane's tab group (the same layout slot) at the shown pane's measured size, and follows that size when it changes.
- Those tabs are hidden as before once their group stops being shown: another Screen is shown, another slot is maximized, or the window shows a Screen of another connection.
- A background tab whose group's shown pane has no measured size is not reported; one already reported keeps its report until that size is measured (a tab switch, a route refresh), so a tab switch sends no `hide` in between.
- Against a server without `pty-size-v1` nothing changes: background tabs are not resized there.
- The web client is unchanged and keeps reporting shown panes only.

Out of scope: the server, the protocol, the web client, Screens that are not shown, and the native view code (a view takes its PTY grid from the server's `size` event already before its first layout).

## Capabilities

### New Capabilities

None.

### Modified Capabilities
- `terminal-sessions`: Client Size Reports allows a client to report the background tabs of a shown pane's tab group at the shown pane's size.

## Impact

The native client's per-connection size reporting (`crates/splitty-client-core`, the source driver and the layout model's slot lookups). No wire change: the client sends `resize` and `hide` as before, for more sessions.

## Architecture impact

none

## Decisions

- BTS-1 (provisional, tbd): background tabs of a shown group report the group's size, so under `smallest` they bound the size for another client that shows the session, and under `active` they count like a shown pane of this connection. Reason: the tab already has the window's size when the user switches to it, so the switch resizes and redraws nothing.
- BTS-2 (provisional, tbd): the web client keeps reporting shown panes only. Reason: since v0.4.0 the web client proves the client/server separation and gets no new client behavior.
- BTS-3 (provisional, tbd): a group whose shown tab is not a terminal (a viewer) leaves its background tabs hidden. Reason: there is no measured terminal size to follow, and a viewer's area in cells is not a terminal's.
