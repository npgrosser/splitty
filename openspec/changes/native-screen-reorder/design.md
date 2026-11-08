# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. The current workspace reducer replaces `screen_order` for `ReorderScreens`, filtering unknown IDs. Both `workspace-model` and `Store::workspace_catalog` deliberately retain partial-order semantics; the latter deduplicates at first occurrence without appending unlisted Screens. A native full-list reorder captured before another client creates a Screen could therefore remove that new Screen from the visible catalog without deleting its stored state.

The web `screenMuxHooks` sends full-list reorders only to the local connection. Remote ordering is the order of that window's references. Native `SourceWorkspace` already has the corresponding distinction: local `catalog` follows authoritative workspace membership/order, while remote `catalog` holds opened references and `live_catalog` holds the host's observed directory. Remote catalog refresh updates reference metadata without replacing order; connection reset retains the opened list. `WindowWorkspace::opened` groups those lists and also supplies navigation order.

The existing ordered session protocol already supplies mutation delivery, failure events, other-client invalidation and a post-operation `mux.workspace_resync`. No new acknowledgement protocol or backend receipt is needed.

## Goals / Non-Goals

**Goals:** one small shared server intent for relative local ordering; one UI-free native admission path; remote reference arrangement without remote mutation; selection/session preservation and bounded gesture state.

**Non-Goals:** a second authoritative local order, a generic list-operation framework, remote global rearrangement, host-group sorting, new restore persistence, or a second terminal/layout renderer. Other active native changes retain their own acceptance checklists.

## Decisions

### NSR-1: Add a relative operation and capability

Add workspace `Op::MoveScreen { screen_id, before_screen_id }` in the existing mux capability, with `move_screen` on the existing `mux.op` and control API. The envelope's `screenId` names the moved Screen; optional `beforeScreenId` names an anchor, and omission/null means the current end. Non-string non-null anchors must fail decoding rather than silently become end placement. Keep both server entry points mapped to the same pure operation and rejection behavior.

Advertise `screen-placement-v1` from `ServerEvent::mux_welcome`. Native local placement requires it plus `workspace-catalog-v1`; there is no fallback to `reorder_screens`. Existing clients and the legacy operation are unchanged.

Alternatives considered:

- Making `reorder_screens` append omitted IDs changes its specified partial-order contract.
- An optional expected-order field still needs a new capability because older handlers ignore unknown fields. It also transmits complete lists and rejects unrelated additions which relative placement can safely retain.
- A workspace-wide order revision/receipt introduces extra snapshot and protocol state without improving this host-local relative intent.

The guarantee covers the new operation. An older client can still deliberately submit a legacy replacement order; this change does not reinterpret that client's intent.

### NSR-2: Resolve against current order without restoring hidden membership

Under the existing atomic store dispatch, validate that the moved ID and any anchor name existing Screens and occur in the current order. A valid self-anchor is a no-op. Otherwise remove all entries equal to the moved ID and insert it once before the first anchor entry, or at the current end. Leave all other entries untouched, including duplicate entries and legacy unlisted Screen records. Do not normalize the whole workspace or append the keys of the Screen map.

This narrowly collapses only duplicate entries of the Screen being moved. No Screen document, revision, active identity or session changes, and the effect list is empty. Invalid anchors or moved Screens reject before any committed change. Concurrent creation or other valid reorder changes are retained because the intent is evaluated against the state at dispatch, not an earlier full-list snapshot.

### NSR-3: Keep local and remote order in their existing owners

Use a native command shaped as `MoveScreen { screen: ScreenKey, before: Option<ScreenKey> }`, captured with the local window lifecycle `SourceToken`. Derive identity from keys, never mutable row indexes. Validate same-source keys and current membership again in the worker. This command must not require the dragged Screen to be selected.

For a local Screen, the existing local `SourceRuntime` delegates to a narrow `SourceWorkspace` method. Check current connection, capabilities and loaded ordered catalog, then use a bounded `Client` command for the new operation. Advance the source mutation watermark only after successful send admission and mark the catalog dirty. Existing `flush_catalog` retains the refresh request through sender pressure; an earlier in-flight response is followed by a new request ordered after the operation. Keep the last authoritative order until accepted observations replace it. Existing `mux.op_failed` handling reports rejection and refreshes; no mutation is retried. No new pending success/focus continuation is needed because rearranging does not navigate or claim an acknowledged outcome.

For a remote Screen, `WindowWorkspace` validates the local owner and opened same-source references, then changes only that source's opened `catalog`. Use reference membership, not live model availability: missing and offline references are still arrangeable. The worker publishes the changed workspace without sending any remote operation, changing browse demand, connecting the host or changing the live catalog. Existing refresh/reset behavior retains the arranged order. The command remains valid across a host reconnect if its stable source and references survive; local owner replacement or source disposal revokes it.

Neither path changes `selected`, per-source `shown`, pane/terminal identities, route generation, activity or visible-size ownership. `WindowWorkspace::opened` continues to determine group boundaries and navigation order; host metadata/order is untouched. Adding a separate per-view order array would create unnecessary reconciliation and is excluded.

### NSR-4: Capture one Drawer gesture; keep geometry transient

GPUI owns only drag state, current insertion feedback and keyboard/menu interaction. Use stable Screen keys for row identity. Keep the captured local owner and dragged key for the gesture; derive a before-anchor from currently measured rows in that same source group. Release emits at most one semantic command. An unchanged drop emits none.

Cancel before admission on Escape, window deactivation, owner/source removal, unavailable destination or stale viewport/rem geometry; never reinterpret a cross-group drop as an end placement. A catalog change that adds another Screen is not itself a reason to discard valid identities, but hit geometry must be refreshed or invalidated before drop. Rename/alias changes do not retarget an action. Header/menu clicks and a drag must not also select the Screen or reach a terminal.

Use the existing Screen menu for keyboard-accessible Move Up/Move Down intents, resolving adjacent stable keys in the same group and using the same core admission. Keep disabled availability honest for older local services. Reuse Kit's existing drag/event and semantic styling primitives; no new general interaction framework or presentation state in render callbacks.

Screen sliding belongs to `native-workspace-motion`: a reordered index with unchanged selected identity must not start navigation motion. This change does not implement outgoing Screen painting or duplicate terminal surfaces.

## Architecture impact

- `architecture/core.c4`: extend `splitty.core.mux`'s ordering semantics and `splitty.core.protocol`'s capability description; retain pure reducer/effect boundaries.
- `architecture/server.c4`: existing `client_session` and `service` decode the same new workspace intent through their existing `splitty.core.mux` relationships. Update descriptions only where meaningful; no new authority, gateway or process dependency.
- `architecture/client-core.c4`: extend `connection` for the bounded relative command, `client_workspace` for local capability/admission and remote reference arrangement, and `native_workspace` for captured window-owned routing. Keep the existing source/connection/transport relationships; document actual imports/tests if they change.
- `architecture/gpui.c4`: extend `gpui_workspace`'s Drawer gesture/menu description over the existing client-core seam. No UI dependency enters core or the reducer.
- Update `docs/architecture.md`'s mux operation, Client connections and Remote Screens concepts, and `docs/protocol.ts` for `move_screen`, `beforeScreenId` and `screen-placement-v1`. Record focused native evidence in `docs/gpui-spike.md` without closing broader platform/remote obligations.

## Risks / Trade-offs

- An anchor may disappear after local admission → reducer rejects atomically; refresh current order and never fall back to a numeric position or replay.
- An older local backend lacks the operation → disable local placement with an explanation; remote reference ordering remains available.
- Local result appears after an ordered refresh → retain authoritative display while awaiting it; immediate drag feedback is presentation only.
- Existing legacy writers can still replace partial order → keep that compatibility boundary explicit; web migration is separate scope.
- Drag geometry changes with scrolling, rem size or concurrent entries → bind geometry to current layout, invalidate stale targets, and cover non-default geometry in UI tests.
- Remote references are not new persistent preferences → retain them across existing refresh/reconnect only; native restart persistence stays with its existing future scope.

## Migration Plan

No stored-format migration. Add the operation and capability first, then client-core admission/reference arrangement, then native presentation. New clients enable local placement only after observing support. Removing the UI action leaves existing server state and remote references valid; the legacy wire operation is unchanged. Targeted checks precede completion; the full workspace merge gate remains deferred to the later merge checkpoint.

## Provisional decisions

- **NSR-1** (2026-11-08, provisional, tbd): additive `move_screen` and `screen-placement-v1` express relative intent; never use a legacy full-list fallback.
- **NSR-2** (2026-11-08, provisional, tbd): collapse duplicate entries only for the moved Screen, preserve all other order entries, and reject existing but unlisted moved/anchor Screens.
