# Design

## Context

See `proposal.md` for motivation and exclusions. The existing `client-connections` contracts already require fixed backend/generation identity, stale callback isolation and retained local terminal state; this design changes implementation, not those contracts.

`Client` assigns a connection UUID and retains charged ordered events. `LocalWorkspace` owns the one local catalog, canonical `ScreenModel`s, route admission and close assessments. `native_workspace` owns the service and independent session, settings and hub streams. GPUI retains pane views and measured geometry. Some boundaries already capture connection identity (documents, close plans, transfers, floating actions and API jobs); others still use bare identifiers (`Input`, `Split`, `Visible`, `Attached`) or read a mutable current route generation in a retained terminal callback.

GPUI's pane map currently keys by pane ID; split/slot maps key by node ID and floating order by Screen ID. These keys are sufficient only while there is one source. `WorkspaceRefresh` increments the existing route generation after a coherent cross-Screen observation; it must not become a new reason to reject normal input for an unchanged attached session.

## Goals / Non-Goals

**Goals:** Make identity explicit at existing ownership boundaries, reject foreign or superseded delivery before effects, preserve current local behavior, and leave a tested seam for later remote sources. Maintain session/view retention during same-source moves and preserve API stream separation and cleanup evidence.

**Non-Goals:** No production multi-source registry, remote model, source selection UI, host opening, reconnect state machine, persisted identity or generic transport abstraction. Do not introduce another mux tree, change server identifiers, or rewrite already-correct document/API internals solely for naming consistency.

## Decisions

### SR1: Distinguish stable object keys from execution authority

Add a small nested identity module under the existing client workspace module, with deliberate re-exports for GPUI. No new crate or top-level module is needed. New seam types have private fields and readers; construction that assigns authority stays with the real model/runtime owner.

| Type | Minimal content | Purpose |
| --- | --- | --- |
| `SourceId` | Opaque identity anchored to the first assigned connection UUID | Stable namespace for one source in this worker; retained if that source's connection changes |
| `SourceToken` | `SourceId` and the current existing connection UUID | Immutable execution/delivery authority |
| `ScreenKey` | Source and Screen ID | Screen selection, retained state and targeting |
| `PaneKey` | Source and pane ID | Retained terminal/view identity independent of its Screen position |
| `LayoutKey` | Screen key and node/slot ID | Screen-local geometry, tab scrolling and element identity |
| `TerminalToken` | Source token, pane ID and exact session ID | Input/reply/attachment routing to the existing admitted binding |

`SourceId` is not a host name, backend selector, authorization grant or persistence key. Only the existing local source is instantiated in production. The assigned worker command channel already identifies its window; no new window UUID is necessary. The initial connection UUID can supply the source namespace without generating another UUID. Source identity and current connection identity are distinct concepts even when their first values coincide. A future connection replacement preserves the source key but replaces its execution token; replacing the worker uses its new channel and source.

Reuse current counters rather than adding an all-purpose epoch: connection UUID rejects replaced connections; exact pane/session pairing rejects replaced bindings; coherent route generation fences geometry/reconciliation; Screen revision protects captured structural gestures; request IDs identify pending operations; document version protects content. Terminal-engine generation continues to handle emulator reset/gesture lifetime. None is silently substituted for another. Do not add an attachment counter merely for synthetic tests; removal revokes current routing, and any same-identifier re-admission must still satisfy the actual current source/connection/session binding and existing attachment state.

Internal maps confined to a single `ScreenModel` or explicitly owned `LocalWorkspace` can retain bare canonical IDs. Qualify identifiers where they cross owners or enter retained cross-Screen view state. Avoid a generic keyed-object framework or wrappers around every wire field.

### SR2: Admit immutable intents at the worker boundary

The worker validates a command's captured source and connection before looking up a target or serializing any operation. It then performs the existing membership, visibility, revision, capacity and consent checks. Never recover missing authority from the currently focused Screen or upgrade a queued token to the latest connection.

| Boundary | Required capture and validation |
| --- | --- |
| Create, switch, cycle and rename Screen | Source token; exact Screen key where applicable; relative navigation still resolves its next step using current order on that source |
| Split, tab, profile tab, new terminal, selection and focus | Source token and exact existing Screen/pane/session target appropriate to the operation; preserve shown-Screen checks |
| Input and emulator-generated replies | Immutable terminal token plus current exact attachment check; hidden surviving terminals remain allowed to send live replies |
| Divider resize, maximize, visibility and terminal sizes | Source token, captured Screen, existing route/structural freshness and measured target; apply existing shown/visible checks |
| Placement, transfer, float/dock and template application | Add source ownership to existing immutable target/plan types; retain their current revision, placement and impact validation |
| Close request, prompt resolution and outcomes | Captured source token, request ID and assessed identities; retain current impact revalidation and harmless-metadata tolerance |
| Local settings, zoom, profiles, templates and passive Hosts | Keep their fixed local API owner and existing request/generation checks; bind the UI command/delivery to that local source without widening API authority |

Eliminate stringified connection IDs in touched command variants. Raw protocol IDs remain unchanged when the owning worker finally calls `Client`. Source identity is client-owned context, never decoded from a peer payload.

Startup window close remains a special channel-owned lifecycle intent: there is no ready source token yet. Preserve its existing unresolved-impact assessment and exact request matching. Do not mint a fake connection to handle startup or make shutdown contingent on a model existing.

### SR3: Scope delivery without duplicating event payloads

Workspace publication exposes its assigned source and connection before dependent feature deliveries. Add source context to the workspace worker's session-event envelope and attachment notifications; verify it against the underlying event's existing connection UUID. Keep `connection::Event` transport-focused and charged rather than cloning its JSON/output into a new unaccounted queue.

GPUI checks current source/connection before reading an event body or looking up a pane/session. An attachment notification identifies its source and session and resolves only inside that source's current admitted routes; it never broadcasts by bare session ID. Results from pending operations additionally retain their existing request/feature generation checks. Lifecycle status and cleanup completion remain owned by their worker channel, not routed through whichever Screen is active.

Document reads already enforce connection and request/version identity in `document.rs`. Preserve and regression-test those checks. The current local workspace worker has no document-read command path; document rendering belongs to the standalone probe. Do not invent a workspace document path for this prerequisite. A later workspace-owned document result or annotation callback must retain source/Screen/pane context alongside the existing document identity instead of attaching to the current selection on completion. The standalone probe remains single-source and does not gain a registry or new viewer functionality.

### SR4: Keep views stable while replacing obsolete callbacks

Key pane views and their search/focus ownership by `PaneKey`; store the bound connection/session alongside each view. Key Screen-local floats and selection by `ScreenKey`, and node/slot bounds, scroll state, drag targets and repeated element identities by `LayoutKey`. Preserve existing zoom/measurement invalidation as well as these keys. Same-source movement between Screens preserves the pane key and emulator. A replaced connection/session cancels its old gesture state and cannot inherit executable callbacks merely because its pane key matches.

Remove the terminal callback's shared mutable current-authority lookup. Input callbacks retain their terminal token. Geometry callbacks retain the relevant current layout/visibility capture when installed or measured. When a surviving view needs updated geometry routing, replace its callback binding at explicit reconciliation while preserving the emulator and search state; any older closure or queued command still contains its original token. A crate-private terminal callback setter is acceptable if required by this real production use; no generic event-bus abstraction is needed.

Pending resize coalescing stores the complete original capture with the dimensions. Retrying a full-queue resize cannot attach a new connection, Screen or route generation to old measurements. Discard stale measurements and obtain a fresh measurement from the accepted visible layout. Ordinary queue retry retains current coalescing behavior. No mutation/input retry is introduced.

### SR5: Keep admission and lifecycle invariants intact

Identity wrappers add no unbounded registry, input backlog or replacement queue. Keep the current bounded command/update channels, attachment and pending-pane limits across all local Screens, charged event leases and snapshot ownership. Foreign/stale command rejection occurs before a mux operation, terminal input, capacity-reserving creation or API job. Dropping a rejected delivery releases its existing charge normally.

Preserve ordered replay/live delivery, suppression of replay-generated terminal replies, exact session routing, same-source transfer reconciliation, and normal window-close acceptance before shutdown. Source qualification must not cause extra attach/detach, activity claims, service opens, host calls or changes to hub/settings polling and cleanup.

### SR6: Follow-on remote boundary

A separate change may assign additional source IDs to explicitly opened host streams and associate them with exact validated server target identities. It must separately own live catalogs, explicit opened references, reconnect and persistence. This change supplies namespace and execution checks only; it does not prepopulate a remote model or add dormant remote commands. `LocalWorkspace` retains its current local catalog/creation policy and is not generalized into remote catalog ownership.

## Architecture impact

Update descriptions and relevant relationships for `splitty.client_core.client_workspace`, `native_workspace`, `connection` and `client_document` in `architecture/client-core.c4`, and `splitty.gpui.gpui_workspace`, `terminal` and, only where touched, `gpui_document` in `architecture/gpui.c4`. The existing `native_workspace -> client_workspace` and `gpui_workspace -> client_workspace` boundaries carry the new shared keys/tokens; `gpui_workspace -> terminal` owns stable view identity and callback rebinding. Nested identity code stays within `client_workspace`, so it adds no top-level component. Do not add dependencies on server/SSH code to GPUI or upward dependencies from client-core.

Update the **Client connections** concept section in `docs/architecture.md` to explain stable source keys, immutable execution capture and local-only scope. The server mux/reducer/effect ownership and native transport wiring do not change. No protocol documentation or wire schema update is expected; if implementation discovers a required wire change, it is outside this prerequisite.

## Risks / Trade-offs

- Excessive token invalidation interrupts hidden terminals or same-source moves → scope terminal input by connection and exact binding; use route/Screen freshness only where geometry or structural intent needs it.
- Retained callbacks silently adopt fresh authority → capture immutable values, retain them in pending resize state, and test invocation after replacement rather than only testing equality helpers.
- Identity fields spread across all feature DTOs → qualify the workspace ownership seam and keep established feature owner/request guards where they already suffice.
- Synthetic-source tests imply implemented multi-host behavior → use synthetic namespaces only in pure routing/view-key tests; integration tests exercise the existing local service.
- Preparatory change becomes a general workspace rewrite → retain the existing local model and worker, extract only the small key/admission helpers actually used in production.

## Migration Plan

Introduce keys and captured authority in client-core, thread them through the local worker, then migrate GPUI retained state and callbacks. Keep the change independently buildable and reviewable without remote features. Extend current tests at each seam and run the complete targeted suite after integration. No user data migration is required; reverting the code restores the previous local implementation without changing wire or saved formats.

The experimental branch completes with targeted tests/Clippy, formatting, architecture checks and OpenSpec validation. Full-workspace merge checks and Windows builds are deferred as described in tasks; no new release claim follows from this refactor.

## Provisional decisions

- **SR1 (2026-11-07, provisional, tbd):** Use an opaque source namespace anchored to the first assigned connection UUID, with existing current connection UUID as execution authority; no additional window identity or host enum.
- **SR2 (2026-11-07, provisional, tbd):** Retain existing route generation, structural revisions and request/document identities for their specific purposes; do not add a universal epoch or invalidate unchanged terminal input on every catalog refresh.
- **SR3 (2026-11-07, provisional, tbd):** Use spec-driven planning with `skip_specs: true`, since this cross-component prerequisite preserves existing behavioral contracts.

These bullets are ready for integration into the central decision log; planning edits for this change are confined to its own directory.
