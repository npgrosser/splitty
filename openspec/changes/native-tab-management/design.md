# Design

## Context

See proposal.md for motivation and scope. Canonical layout and sessions already live in the service. Native layout gestures capture connection, Screen, structure and pane/session identity; the UI renders snapshots. `move_pane` appends and activates even inside one slot. `arrange_panes` replaces group identities and is unsuitable for exact native insertion. The local workspace already maintains an ordered catalog mutation watermark for close settlement. The current tab strip clips overflow; header targets do not express tab order.

## Goals / Non-Goals

Keep single-pane placement pure and precise, preserve sessions and current unrelated state, and reuse ordered verification without optimistic duplicate layouts. Input geometry and scrolling belong to GPUI; layout semantics and pending intent belong to the UI-free client model. No revision CAS, global layout lock, mutation replay, WASM extraction or browser docking migration is introduced.

## Decisions

### Separate exact placement operation

Add `move_tab { paneId, sourceSlotId, targetSlotId, beforePaneId? }`, gated by `tab-placement-v1`. Omitted anchor means append. Validate every identity before mutation. Same-slot reorder keeps IDs, weights and the effective active pane, including implicit first-pane selection. Cross-slot movement removes the source, collapses an empty slot, normalizes surviving current sibling weights, inserts at the exact destination and activates the moved pane. It emits no process effects. Leave `move_pane` and its edge splitting semantics intact. An optional anchor on that operation would ambiguously overload legacy append/activate and directional fields; replacing the arrangement would overwrite unrelated state and regenerate IDs.

### Ordered placement settlement

Keep one connection-wide pending placement in LocalWorkspace. Reserve a mutation watermark before submission and commit it only after admission succeeds. A same-owner catalog response whose captured in-flight watermark covers the operation is the serial fence. Older responses cannot settle it. Retain matching operation failures until that fence so a late failure cannot affect a newer placement. At the fence reject a recorded failure, malformed catalog or missing Screen; otherwise mark the Screen dirty. The first matching, actually accepted snapshot after the fence settles exact immediate-before/end placement and pane/session identity, or reports supersession/verification failure. It need not be uniquely correlated to a resync request: stream order proves it post-operation. Queue-pressure refresh remains dirty and retries; mutations never retry. Screen selection or local tab navigation cancels deferred focus only, keeping settlement alive. Same-group reorder preserves selection/maximization; a confirmed cross-group placement may focus the moved pane only if no later navigation superseded that choice.

### UI translation

Use a fixed-height tab row, horizontally scrollable nonshrinking chips, a pinned add action, a distinct header insertion target and a thin insertion marker. Pointer midpoints map to before-anchor or append; body edges keep existing split moves. Reveal the selected tab after keyboard selection and placement. Self or identical placement is a client no-op; backend self-anchor remains invalid. Escape cancels the gesture. The worker validates captured connection/Screen/slot for `CycleTab` and clamps Previous/Next against its current selection, independently of stale UI indices. Use provisional Mod+PageUp/PageDown; editing and close dialogs have priority.

## Architecture impact

Existing `splitty.core.mux` gains pure intent validation/reduction; `splitty.core.protocol` advertises the capability. Existing `splitty.client_core.client_workspace` owns exact placement, catalog fence and bounded group navigation; `native_workspace` routes captured commands and publishes settlement. `splitty.gpui.gpui_workspace` owns insertion geometry, scrolling and shortcuts. Update their component descriptions/relationships in architecture/core.c4, client-core.c4 and gpui.c4 where the responsibilities changed. No new component or dependency is needed. Extend the native workspace/layout discussion in docs/architecture.md and the shared wire model in docs/protocol.ts plus frontend protocol types; the web arrangement implementation stays unchanged.

## Risks / Trade-offs

- Uncorrelated failures and snapshots → only one pending placement, ordered catalog watermark, accepted post-fence snapshot and adversarial ordering tests.
- Header/body drop ambiguity and clipped geometry → separate target kind, visible viewport clipping and actual-window insertion/overflow checks.
- Implicit active tabs and source collapse → reducer fixtures assert effective selection, identity and nonuniform current weights.
- Concurrent edits after admission → validate current source/anchor, settle superseded intent without replay; this is not a revision lock.
- Focus theft → later local navigation cancels deferred focus while verification continues.

## Migration Plan

Ship the additive operation on the experimental native branch. Older services retain their existing supported gestures; exact placement is disabled without the advertised capability. The web client continues `arrange_panes`. Rollback removes the new native affordance and capability without an on-disk migration. Targeted tests and owned native-window evidence gate integration; broad CI and release/platform certification remain deferred.

## Provisional decisions

- NT-1 (2026-11-07, provisional, tbd): separate root-only exact `move_tab` with a before-anchor and `tab-placement-v1`, preserving legacy operation behavior and deriving source proportions from current server state.
- NT-2 (2026-11-07, provisional, tbd): clamped focused-group Mod+PageUp/PageDown, scrollable tabs with selected-tab reveal and header insertion targets; same-group reorder retains local selection and maximization.
- NT-3 (2026-11-07, provisional, tbd): reuse the ordered catalog watermark plus accepted post-fence Screen snapshot for bounded one-at-a-time exact placement settlement, without a new request acknowledgment protocol.
