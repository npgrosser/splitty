# Design

## Context

See proposal.md for motivation. `ScreenState` already owns floating entries, their bounds and roots. Session routing and part of selection reconciliation traverse those roots, but native presentation, focused-slot lookup, maximization, size claims and geometry bookkeeping currently assume the tiled root. The service supports floating tab groups, selecting/new tabs/closing within them, whole-group docking and changing bounds. Split, divider resize and exact tab-placement operations are intentionally root-only; template application rejects floating split layouts.

Two shared defects are relevant: float/move validation ignores bounds while templates check finite values and positive dimensions; closing a single-pane slot within a nested floating tree deletes the entire entry instead of preserving its siblings. Neither requires a new protocol or expanded creation semantics.

## Goals / Non-Goals

Project the existing complete canonical layout with consistent identity, focus and visibility. Keep transient stacking and gestures local. Preserve supported server semantics rather than introducing a second layout authority. Do not add a docking framework, nested-float creation protocol, OS popout windows or template UI in this increment.

## Decisions

### Reuse canonical roots and existing operations

Add borrowed `presented_floats()` alongside `presentation_root()`. With no maximized slot, the latter remains the tiled root and the former exposes all canonical floats. With a maximized slot anywhere in the Screen, borrow that slot as the sole presentation root and expose no floating surfaces. Selection, focused-slot lookup, displayed-pane checks, tab cycling and pending new-tab focus must use the same traversal. The existing entity registry remains keyed by pane/session identity.

`PaneActionTarget` accepts a terminal in any canonical root for Screen transfer. Floating a tab separately requires tiled membership and more than one remaining root pane. `FloatingTarget` captures connection, Screen, float ID, original bounds, structural root identity and exact pane/session membership. Metadata and active-tab changes do not stale the capture; bound, structure or route changes do. Commands are identified FloatPane, DockFloat and MoveFloat intents. Dock returns the entire captured group to the server's first root slot, including its existing empty-root fallback. No unused destination-picker abstraction is added.

One pending floating action is mutually exclusive with unresolved placement, transfer, close or route collection. Admit once, retain rejection until an ordered catalog fence and a subsequent accepted Screen snapshot can settle the result, and emit an identified confirmed/rejected/unverified outcome. Integrate with coherent workspace refresh so an external transfer cannot strand settlement. No mutation retry or optimistic canonical layout is introduced. Deferred focus for float/dock is canceled by later local navigation; geometry movement does not steal terminal focus.

### Render overlays as presentation

GPUI renders the tiled root first in a relative content viewport, then floating roots as absolute children with complete hit occlusion. Reuse the existing node/tab/terminal rendering with explicit surface context. A small per-Screen float-ID order controls local stacking and is pruned against canonical entries. Raising a group changes no server layout. A float header supplies drag, Dock and Maximize controls; a resize grip changes its rectangle. Float Tab operates on the focused tiled terminal. Existing transfer menu ordering remains intact.

Ordinary floating tab selection, cycling, new tabs/profile choice, close, search and cross-Screen transfer work through existing paths. Root-only split, divider and tab drag/drop controls are unavailable on floating surfaces. Canonical nested floating roots, if observed, render defensively and support non-structural actions and safe close; this does not enable unsupported creation or split mutations.

### Keep geometry finite and gestures cancellable

Shared validation matches template semantics: all four bounds are finite; width and height are positive. Negative/offscreen coordinates and dimensions greater than one remain valid, because existing web layouts use them. Share the pure predicate between template and direct-op admission. Reject invalid direct operations before mutation or effects. Closing a floating subtree removes only the affected pane/slot, collapses single-child splits and removes the entry only when no panes remain.

Native display clamps projected rectangles to a reachable viewport with usable header/body geometry, including legacy unusable bounds, without rewriting stored bounds on resize. Convert bounded values before passing pixels to GPUI. A drag captures its canonical target, displayed starting rectangle, viewport and zoom, and retains only a temporary rectangle. It submits one finite normalized MoveFloat on release only when changed. Escape, window deactivation, Screen/owner change, viewport/zoom change or incompatible authoritative update cancels it. Paint-scoped window mouse capture follows a held gesture across other overlays; headers and grips consume their input without sending terminal bytes. Overlap order controls mouse, wheel and terminal focus consistently.

After accepted geometry/layout changes, displayed terminals measure current bounds and publish size claims. Hidden tabs, other Screens and groups hidden by maximization retain output processing without visible size claims. Existing route-generation invalidation continues to protect cross-Screen transfers.

## Architecture impact

Update `splitty.core.mux` in architecture/core.c4 for shared finite floating bounds and safe floating subtree removal. Extend `splitty.client_core.client_workspace` and `native_workspace` descriptions in architecture/client-core.c4 for canonical surface projection and captured floating settlement, and `splitty.gpui.gpui_workspace` in architecture/gpui.c4 for overlay geometry and local stacking. Existing dependency relationships suffice unless implementation adds a directly used module edge. Update the workspace and Client connections concepts in docs/architecture.md. No top-level crate or library dependency is planned.

## Risks / Trade-offs

- Overlays can leak events to terminals behind them → occlude complete surfaces and test exact raw input on overlapping terminals.
- A stale drag can overwrite a later layout → revalidate captured identities/bounds/structure, cancel obsolete geometry and never replay mutations; this remains admission validation, not a server CAS protocol.
- Oversized legacy rectangles can overflow native pixel types → bounded display projection, no automatic state rewrite and authoritative validation for new values.
- Supported actions differ between root and float → explicit surface context and disabled root-only controls, with shared model admission as final authority.
- Complete projection touches existing navigation and close paths → extend existing model/runtime tests and run the real-window workspace smoke as well as float acceptance.

## Provisional decisions

- **NFG-1** (2026-11-07, provisional, tbd): Dock uses the first tiled slot; explicit dock-target selection can be added later without changing the shared operation.
- **NFG-2** (2026-11-07, provisional, tbd): floats keep per-window stacking and a reachable clamped display rectangle; viewport changes never persist new canonical bounds automatically.
- **NFG-3** (2026-11-07, provisional, tbd): create and manipulate the currently supported floating tab groups; keep split/divider/tab-placement operations root-only while safely projecting any nested floating roots already observed.

## Migration Plan

Keep the current accepted preview until a new frozen release candidate passes targeted and real-window checks. Bounds validation tightens only invalid new operations and does not rewrite saved layouts. Rollback can restore the prior preview; the web client and its valid operations remain compatible. Platform acceptance remains Linux until separately verified on Windows/macOS.
