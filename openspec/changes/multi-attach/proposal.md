# Proposal

> Draft migrated from `roadmap/101-multi-attach.md` on 2026-10-09. Not planned yet. Specs and tasks are written when this change is next. Depends on: a fix for the PTY reader that is lost when a session's stream is cancelled (critical finding in `docs/issues/openspec-baseline-findings.md`); relates to the open layout-sync decision in `docs/layout-sync-drift.md`.

## Why

Several clients on the same screen at once serve three use cases: pair programming in the same terminal, watching someone work (mentor and trainee), and the same screen on several devices (desktop and tablet). The roadmap doc deferred this because of the single-attach and ownership model. That model was removed in March 2026 (`fa5e5b7`): every connected client can already read and modify every screen (`mux-protocol`, Concurrent Clients Without Screen Ownership). Concurrency is possible but not reliable. A second client is not a trustworthy view of the same screen:

- Layout ops that start or end no session (rename, resize, move, float, `set_active_screen`) are not propagated to other clients, so their layouts drift silently.
- Broadcasts are ordered only within one op; events from separate ops can reach a client out of order.
- Two connections streaming the same session share one PTY reader: the second stream ends the first with a spurious `closed`.
- PTY size is whatever the last `resize` set; clients with different window sizes overwrite each other (addressed by `pty-size-policy`).
- The client's own layout can drift from the server model with no reconciliation (`docs/layout-sync-drift.md`).

## What Changes

- **Layout sync to every client**: every applied op, not only session-changing ones, reaches all other connected clients, without the originator applying its own op twice.
- **Ordering**: all clients observe changes in the order the server applied them, across ops.
- **Concurrent ops**: conflicts between clients are detected with the existing per-screen revision (reject and resync on mismatch). This turns fire-and-forget into acknowledged delivery for all ops. The op format stays the same; only the delivery semantics change, as `docs/protocol.ts` §8.3 anticipates.
- **Shared output streams**: any number of connections can stream the same session at the same time, each receiving the complete output, and none of them ends another's stream.
- **PTY size policy** for one pane shown by clients with different sizes: done in `pty-size-policy` (2026-10-26): the smallest size among the active clients by default, `latest` and `smallest` configurable, a visible marker on larger clients and a takeover. Roles below no longer need to decide the size.
- **Input routing**: today any connection may send input to any session, as in tmux. The roadmap proposes roles: *primary* (full control, sets the PTY size), *observer* (sees output, cannot type), *shared* (everyone types).

Out of scope:
- Access by different people or accounts. Today every authenticated client acts as the same user with full rights (`access-control`); this draft covers several clients of one user.
- Exposing a server beyond the SSH tunnel (`splitty connect`) and the hosted demo (roadmap/102-cloud-demo.md).
- Reworking how layouts are mirrored from dockview in general; that decision stays with `docs/layout-sync-drift.md`.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- None expected. Client roles might warrant their own capability if they grow beyond a flag on the connection.

### Modified Capabilities
- `mux-protocol`: change propagation for all ops, cross-op ordering, acknowledgement and rejection of every op, possibly roles announced in `mux.welcome`.
- `terminal-sessions`: several connections streaming one session, the PTY size policy, input routing by role.
- `workspace-model`: rejecting an op whose base revision is outdated.
- `access-control`: only if observers get read-only access or several users are admitted. Today any authenticated client has full rights, so this would be a deliberate MODIFIED with a stated reason.
- `control-api`: possibly none. HTTP-applied ops already nudge every client, but they must fit the new ordering and conflict rules.

## Impact

Affected areas: the WebSocket op handling and broadcast path in the server, session streaming and the PTY reader, the frontend's mux sync (applying remote ops to dockview, the resync and snapshot rebuild), frontend resize handling, `docs/protocol.ts` §8 and the Client Autonomy section of `docs/architecture.md`, which still describes single-attach.

`docs/layout-sync-drift.md` names multi-attach as the trigger for option C (`adopt_pane`) and for ACK/REJECT on all ops. Its recommendation should be revisited when this change is next.

Open questions for the design:
- Is the complexity worth it, or is a read-only observer mode the better 80/20? The roadmap alternative "take over the screen and disconnect the other client" depended on single-attach and would now be a restriction to add, not a default.
- Op broadcast, with remote ops applied incrementally in dockview, or today's nudge plus full snapshot rebuild? The rebuild already disturbed panes that were still attaching (`docs/layout-sync-drift.md`, fix 3).
- Echo detection: the roadmap suggests `clientId` plus an op id. There is no op id on the wire today.
- PTY size: decided in `pty-size-policy`; whether roles should override it (an observer that never counts) is open.
- Who is primary, how is that chosen, and what happens when the primary disconnects?
