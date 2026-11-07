## Context

The current `LocalWorkspace` represents one Screen, and the native worker creates that Screen after welcome. Welcome lists screens once; per-screen resync silently ignores deletion. The existing `mux.screens` API executes concurrently with session messages and cannot act as a barrier after a queued mutation. Its API-only receiver also skips session events. Existing lifecycle operations already supply the required service behavior.

## Goals / Non-Goals

Keep one owned service, connection, worker and pair of bounded command/update queues. Separate local presentation from authoritative catalog and screen snapshots, retaining terminal entities across switches. The proposal defines feature exclusions; this increment does not add a general request framework, another dock tree or per-screen workers.

## Decisions

### Ordered catalog on the session stream

Announce `workspace-catalog-v1`. Accept `{type:"mux.workspace_resync"}` in the existing serial mux handler and return `{type:"mux.workspace_snapshot",id,timestamp,screens:[{id,name?}],activeScreenId:string|null}`. Read order, names and active ID under one store read. Deduplicate ordered IDs at first occurrence and omit nonexistent entries; return a null active hint unless it names an emitted entry. This handles existing partial/duplicate order states without changing the reducer. Existing per-screen snapshots and broadcast behavior remain unchanged. The native client gates the request on the capability; absence produces an explicit unsupported state.

Use existing mutation nudge reasons (`mux_op`, `cli_op`), plus screenless or unknown-screen nudges, to mark catalog refresh dirty. A known-screen `session_metadata` nudge only refreshes that Screen. Do not refresh catalog on terminal output or every metadata update.

### Screen model inside a connection-wide workspace

Rename the current per-Screen state to `ScreenModel` inside the workspace module. Add connection-wide `LocalWorkspace` owning ordered `ScreenInfo` entries, per-Screen models, the shown-screen ID, pending screen intents and global admission/routing. Each Screen continues using its canonical `Arc<ScreenState>`; local tab/focus and pending move state stay with that Screen. No copied layout tree is introduced.

Public integration surface:

- `LocalWorkspace::new(connection_id)`; `connection_id()`, `has_pending_create()`; `screens() -> &[ScreenInfo]`, `shown_screen_id() -> Option<&str>`, `shown()/screen(id) -> Option<&ScreenModel>`.
- Existing `snapshot`, `focused_pane`, `selected_pane`, layout and selection methods remain shown-Screen conveniences. All pane creation methods apply connection-wide admission first.
- `session_for(pane)`, `pane_for(session)`, `screen_for(pane)`, `routes()`, `session_ready`, `observe_attached` cover all Screens.
- `create_screen(client,name) -> Result<ScreenIds>`, `switch_screen(client,id) -> Result<bool>`, `rename_screen(client,id,name)` and `close_screen(client,id)`.
- `set_catalog_supported`, `mark_catalog_dirty`, `flush_catalog(client)`, `apply_catalog(connection,value)`; existing snapshot/spawn/failure/reset methods route within this workspace. `WorkspaceDelta` carries added/removed routes and presentation changes.

`ScreenInfo` retains the shared ID and optional-name types. `Client` adds typed `request_workspace`, `rename_screen`, `close_screen`, `set_active_screen` and `create_screen_with_ids`. The reserved-ID create method sends only the mutation: a later refresh failure must not be mistaken for a failed creation admission. The old single-screen convenience can retain its existing create-and-resync behavior for probes.

### Bounded pending lifecycle and catalog sequencing

Reserve a screen's initial pane before sending its create op. Count all known sessions and unresolved pane reservations across every Screen against the existing 128 attachment limit, counting a pending pane with a known route once. Release a reservation on failed send or rejected creation, and release admitted routes on authoritative removal. The attachment bound alone is not a screen-count bound: separately cap known and pending Screens at 128 (NS-1, provisional, tbd, 2026-11-07). Reject additional local creation before submission. An oversized authoritative catalog is rejected atomically with a visible capacity error, preserving the prior model rather than truncating membership.

Keep at most one catalog request outstanding, plus a dirty bit. Each accepted local lifecycle mutation increments a local serial; record that serial when sending a catalog request. Its response can settle only intents at or before that watermark. A create submitted after an already outstanding request remains pending even if the older response omits it; a dirty follow-up query observes it. Preserve pending routes and output before catalog/screen snapshots. Reject delayed snapshots and spawns for removed, unreserved screens. Keep refresh dirty after transient queue rejection; never report a mutation as accepted merely because a later refresh was queued.

The initial catalog uses its valid active hint, then first screen, to initialize local choice. Create the initial Screen only once when initial discovery is empty. A newly created Screen becomes shown after catalog confirmation unless a later explicit local selection superseded that intent. Switching to an existing Screen changes local selection after command admission; later server activation hints never steal it. Closing a shown Screen selects a surviving screen after removal; closing the final one leaves an empty New screen state, without automatic recreation.

### Native lifetime, visibility and editing

Keep one pane-to-terminal entity registry for the union of all admitted routes, including pending creations. Feed hidden output and live emulator replies; replay replies remain suppressed. Render only the shown Screen. A switch hides previous visible sessions, clears obsolete layout bounds and drag state, then reports measured sizes for the new visible panes. Scope callbacks to connection, screen and session; do not gate emulator replies on visibility. Preserve one bounded queue and atomic paste admission.

Changing a catalog name or receiving a hidden-screen update must not call terminal focus unconditionally. Restore focus on explicit navigation or replacement of the focused terminal; preserve focus while a rename editor or close prompt is active. Rename uses a native text editor. Closing a screen uses the existing busy-session metadata to offer one local confirmation when appropriate; no new backend policy is introduced.

## Risks / Trade-offs

- A catalog reply may predate a newly submitted mutation → local mutation watermarks and one dirty follow-up request prevent premature cleanup.
- Hidden activity may accidentally resize or repaint the shown screen → screen-scoped visibility and measured-size guards; retain live replies independently.
- A removed screen may have queued output or UI callbacks → route and generation checks precede delivery; terminal entities are removed only for their own retired routes.
- Catalog-only screens can outnumber attachments → validate retained catalog bounds separately and show a clear admission error rather than partial authoritative state.
- Generic mutation nudges cause some unnecessary small catalog requests → coalescing bounds them; high-rate pane metadata remains Screen-only.

## Architecture impact

Update descriptions in `architecture/client-core.c4` for `client_workspace` and `connection`. Move the UI-free local worker, its typed commands/updates and control reconciliation from the GPUI crate into an explicit `splitty_client_core::native_workspace` runtime module, retaining its behavior and owned-service lifetime. Add that component and its actual dependencies; GPUI imports the runtime directly and retains presentation, geometry and event capture. Remove the old `workspace_session` component from `architecture/gpui.c4` and update `gpui_workspace`. This is native runtime reuse, not a browser-portable kernel or a WebAssembly implementation. Existing relationships to `core.mux`, `core.protocol` and server session handling remain the boundaries; update the map only for actual new relationships. The model stays under the existing workspace component. Update the native workspace/Client connections concept in `docs/architecture.md` and the touched messages in `docs/protocol.ts`.

## Migration Plan

Additive capability and messages leave older clients unchanged. The experimental native app requires the capability for multi-screen control and retains the isolated service lifetime. Land protocol, model and UI together on the native feature branch. Rollback removes the new native controls and catalog consumer; existing service operations remain compatible. No persisted data migration is involved.
