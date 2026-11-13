# Proposal

## Why

The server sends `process.finished`, `process.idle`, `session.exited` and `program_status.changed` as raw facts, and each client derives from them whether a notification is due, its kind, severity and text, and which settings switch governs it. The web client (`frontend/src/lib/notifications.ts`) and the native client core (`splitty-client-core/src/notifications.rs`) do this separately and have drifted: the web client shows no notification for program status reports, shows a notice for the pane the user is watching, and keeps notifications of a kind that was switched off; the native client ignores `userInitiated`. And `userInitiated` itself is only set for a session closed by a `close` request of a connection that does not stream it, so closing one's own pane notifies in both clients.

## What Changes

- Every notification event carries `notice`: `kind`, `severity`, a plain-text `title`, the `notifications.*` switch that governs it (`setting`) and, when no window should notify, why (`suppressed`: `user-initiated` or `program-status`). The server builds it once, with the texts the clients used to build. `program_status.changed` carries a notice exactly when it is marked `notify`; its title names the screen by the server's catalog.
- A session the server closes while its program runs (a `close` request, a closed pane or screen) exits user-initiated; its `session.exited` is suppressed.
- `process.idle` of a session that holds a root program status record is suppressed: the program tells through its record.
- `mux.welcome` lists `notification-notices-v1`.
- Both clients take the notice as given and keep only what the server cannot know: unread state, which pane the window shows, the window's own settings, the host's name in the title, and presentation. The derivation in each client goes. For a server without the capability a client shows a generic notice and no second rule set.
- The web client follows program status notifications and gets the `notifications.programStatus` switch in its settings.

## Capabilities

### New Capabilities

### Modified Capabilities
- `mux-protocol`: new requirements for classified notification events (including when a session ends user-initiated), their capability and how a window takes them.
- `program-status`: the window requirement covers both clients, takes the server's notice, and the watched pane records the notification as read.

## Impact

`splitty-core` (protocol: the notice type and the events that carry it; the notification engine; the PTY session remembers an end it was asked for), `splitty-server` (program status events carry a notice), `splitty-client-core` (notification center reads the notice), `frontend/` (notifications store, toasts, settings section), `docs/protocol.ts`, `docs/architecture.md`.

## Architecture impact

none: the classification lives in `splitty-core`'s protocol module, which every component involved already uses; no component or dependency between components changes.

## Decisions

- **SCN-1 (provisional, tbd):** a session ends user-initiated when the server closes it through its backend while its program still runs, whatever asked for it (a connection's `close`, a closed pane or screen). The engine's separate mark for a `close` request goes. A suppressed exit adds nothing in a window: no record, no notice, no system notification.
- **SCN-2 (provisional, tbd):** `program-status` suppression applies to `process.idle` (and to the generic `notification` event, which the server does not send today). Bells stay no notification in either client: no bell indicator is designed, so there is nothing to suppress there.
- **SCN-3 (provisional, tbd):** a notification for the pane the user watches is recorded as read and shows no notice, for every kind; the native window used to drop program status notifications for it instead, which made no visible difference.
- **SCN-4 (provisional, tbd):** the `notifications.*` switches apply to arriving events; notifications already in a window stay until read or dismissed. The native window used to drop them when a switch went off.
- **SCN-5 (provisional, tbd):** the server names the switch and a window applies its own settings, those of its local server, also to host events, as both clients did; the server does not filter by its own settings. Alternative: a host's settings govern its notifications.
- **SCN-6 (provisional, tbd):** for an event without `notice` (a server without `notification-notices-v1`) a window adds an `info` notification titled `Activity in <process>`, or `Session activity`, governed by `notifications.enabled` only; `userInitiated` is not read. `program_status.changed` without `notice` adds none.
- **SCN-7 (provisional, tbd):** the title of a program status notice names the screen by its name, else `Screen <n>` by its place in the server's catalog. A window used to call an unnamed host screen `Unnamed Screen`.
