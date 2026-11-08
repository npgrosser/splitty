# Design

## Context

See [proposal.md](proposal.md). The server already emits `process.finished`, `process.idle`, `session.exited` and `notification` as facts. `SourceRuntime::event` rejects a connection ID or source token that no longer matches the worker's source. `publish_source_effects` sends a changed model before its `Update::Event`; the GPUI window currently consumes only binary output from that event. `WindowWorkspace::route_for_session` resolves hidden sessions but requires `execution_token` readiness, which is stricter than notification-fact admission. `NotificationSettings` is already in the service's `home.get` result, while the current native appearance projection reads only appearance, Drawer and terminal fields.

`crates/splitty-server/src/client_session/handler.rs` subscribes each client to the live notification bus; production does not call `NotificationEngine::replay` (`crates/splitty-core/src/protocol/notify.rs` calls it only in its unit test). `crates/splitty-core/src/protocol/dispatcher.rs::handle_attach` replays terminal bytes, not notification facts. A remote disconnect retains its source workspace (`crates/splitty-client-core/src/workspace/window.rs::disconnect`), whereas `SourceWorkspace::reset_connection` revokes old snapshots and routes until fresh observations (`crates/splitty-client-core/src/workspace/catalog.rs`). A logical session can therefore survive disconnection. Mux screen creation, split and template application allocate fresh session IDs (`crates/splitty-core/src/mux/reducer.rs`, `mux/templates.rs`), and the PTY manager retains an already registered session under its ID (`crates/splitty-core/src/pty/manager.rs`).

The web client has a window-local notification list, connection-scoped deduplication, watched-pane admission, a 100-item cap and Drawer queries. Its DOM focus/visibility checks, browser Notification API, toast rendering and TypeScript store are presentation and platform code, not mechanisms to port into Rust.

## Goals / Non-Goals

**Goals:** One UI-free native client policy for bounded event classification and unread lifecycle; exact-source routing; independent authoritative settings admission; GPUI-only observation of window focus/visibility and Drawer projection. Keep the existing charged event's lifetime short.

**Non-Goals:** New server events or persistence, replay requests, generic browser settings parity, native toast/OS delivery, terminal-emulator heuristics, or replacing the web client.

## Decisions

### NB-1: A window owns one bounded ledger across ordinary source reconnects (provisional, tbd)

Add a `splitty_client_core::notifications` module with a small owned record: source namespace, bounded event ID/title, event kind, severity, captured session ID, optional typed Screen/pane binding, read flag and arrival order. It never stores `Event`, JSON `Value`, arbitrary payload or its transport lease. Admission takes the delivered `SourceToken`, the current `WindowWorkspace`, the event JSON, accepted settings and whether its resolved pane is watched. It requires `event.connection_id == token.connection_id()` and `window.source(token.source_id()).source_token() == token`; it does **not** require `execution_token` or live send authority. Worker validation and model-before-event publication provide this same-source ordering. A queued fact from the current token may thus be admitted while the retained source is offline, but a queued fact from a replaced token is rejected. Resolve a session from the exact source's `pane_for`, `screen_for` and `session_for` relationship, including a retained offline model; never search globally. Duplicate identity is `(SourceId, event ID)` for retained records, preserving read history if an ID is delivered again, without depending on replay. Badge binding is separately stamped with the current token and captured session. On source removal, remove its records; on replacement of the local service owner, clear the window ledger. Use bounded record and identifier sizes and a 100-record FIFO, with no separate unbounded seen-ID set.

Alternative: key every record by `SourceToken`. That would lose read decisions across an ordinary reconnect and count a repeated ID anew. Another alternative, indefinite seen-ID memory, would violate the window bound.

### NB-2: Notification settings have an independent local read lifecycle (provisional, tbd)

Read only the existing `notifications` object from the assigned local service's `home.get`, using a typed, bounded result through the **existing** auxiliary scheduler, API stream and budget. Add one independently coalesced read job, not another stream or settings owner. Refresh on same-service `settings.changed`. Accept results only for the current local owner and newest generation. Before the first valid result, admit no unread events; after a read failure, continue with the last valid switches and expose a status diagnostic without a corrective write. A new accepted snapshot removes entries whose categories are now disabled; re-enabling does not resurrect them. Validate the six booleans and use the shared service `NotificationSettings` type rather than copying their defaults into GPUI. The web client may continue its own settings implementation.

Alternative: extend `AppearanceSnapshot` with notification switches. The existing read resolves themes after `home.get`; an unavailable theme could then prevent notification settings from reaching the window even when `home.get` succeeded. An independent read preserves failure isolation.

### NB-3: Resolve session association at admission (provisional, tbd)

For a session-scoped event, store its captured `(SourceId, PaneId, SessionId)` plus the source-token and Screen binding available at admission. A generic event without `sessionId`, or an unknown session, may remain as a bounded unscoped record but contributes no Drawer indicator, ever; later discovery of that session does not attach it. No event is inferred from terminal `closed`, output, bell or an OSC sequence. On each coherent model publication, a badge remains bound only if that source still proves `session_for(pane) == captured session ID`; then it may follow that pane to another Screen. Conclusive current-model removal of the pane, replacement of its session, or source disposal prunes its bound records. A reconnect changes the token and resets routes: hide old bindings immediately, but retain at most 100 owned records and their read flags while the new catalog/snapshots are empty or incomplete. Fresh canonical mapping under the current token restores originally bound badges when it proves the same `(SourceId, PaneId, SessionId)`, without any event replay; recompute Screen from that mapping. A missing route during reset/reconciliation is unknown, not proof of removal. This separates historical dedup/read state from current visual association.

Alternative: defer events in an unbounded wait for a future snapshot, or resolve by matching raw session strings across sources. Both risk attaching remote activity to the wrong object. The web client also resolves at arrival.

### GPUI projection and read gestures

The GPUI `Workspace` retains the ledger value and passes a watched-pane observation only when its window is focused, visible, and showing that exact active Screen/pane. It marks the active pane read on focus/visibility return and after deliberate pane activation. The Drawer asks the ledger for each `ScreenKey` count and newest titles, and for each `PaneKey` highest unread severity. The client policy builds remote notification titles from a bounded current host label supplied by the window, never from host-supplied instructions; plain-text sanitization happens before retention. The projected count, severity and accessible summary are text/Kit UI state. Ledger changes call `cx.notify()`; they do not send `Command`, settings writes or persistence. Source authority checks precede any read or view update.

## Architecture impact

- `architecture/client-core.c4`: add `splitty.client_core.client_notifications` for `splitty_client_core::notifications`; add `client_notifications -> connection`, `client_notifications -> client_workspace` for exact source route resolution, and `client_notifications -> splitty.core.config` for the existing settings type. The existing `local_service -> splitty.core.config` relationship includes independent notification settings reads; the DTO stays with the existing local scheduler, so there is no artificial ledger dependency. No server component changes.
- `architecture/gpui.c4`: add `gpui_workspace -> splitty.client_core.client_notifications` for owned unread state and source-scoped Drawer queries. Existing `gpui_workspace -> splitty.client_core.native_workspace` remains the charged event delivery path.
- `docs/architecture.md`, native client section: describe the window-owned, UI-free notification ledger, separate local settings read, watched-pane input, and immediate release of charged events. Preserve the existing web-client notification account as a parallel implementation.

## Risks / Trade-offs

- A replay of an event evicted from the 100-record window can appear again → Bound the dedup horizon explicitly; no unbounded tombstone cache.
- A notification can precede a usable session route → Keep it permanently unscoped for badges; test this ordering and model-before-event publication.
- A reconnect can reuse a pane ID for another session → Hide bindings on token change, then restore only against a fresh canonical exact pane/session mapping; test incomplete snapshots and conclusive replacement separately. The wire has no PTY incarnation ID for deliberate reuse of both pane and session IDs; logical session identity is sufficient for these decorative badges and a new incarnation protocol is outside this change.
- A host can send very large or malformed titles, IDs and bodies → Validate/cap before copying and render only plain text; a malformed event must not stop other event delivery.
- A settings read may fail or lag behind incoming events → Fail closed before the first accepted read, retain last-good switches afterward, and keep the read separate from terminal delivery.

## Migration Plan

This adds only window-local state to the experimental native executable. Existing server data, native transport, web state and persisted settings need no migration. Reverting the client change removes the indicators without changing server state.

## Provisional decisions

- **NB-1, 2026-11-08 (provisional, tbd):** Preserve at most 100 read/dedup records across a reconnect of the same source namespace; hide old badges until fresh canonical mapping under the current token proves the captured source, pane and session, without requiring notification replay. Clear records on source removal or local-service replacement.
- **NB-2, 2026-11-08 (provisional, tbd):** Read notification switches independently from appearance, fail closed until first valid settings, retain last-good on read failure and purge disabled categories on refresh.
- **NB-3, 2026-11-08 (provisional, tbd):** Associate session events only with a route proven at arrival; keep unknown-session events permanently unbadged. Preserve association through a reconnect only with fresh exact source/pane/session proof; treat reset gaps as unknown and prune on conclusive removal or replacement. Deliberate same-pane/same-session-ID PTY reincarnation is outside the wire's identity model.
