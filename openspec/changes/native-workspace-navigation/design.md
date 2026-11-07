# Design

## Context

`ScreenModel` retains canonical server snapshots plus window-local focus and selected tabs. `LocalWorkspace` retains one such model per local Screen. The native worker owns those models and publishes bounded snapshots; GPUI projects the tree and reports the visible panes through the existing visibility/resize path. The web reference maximizes a tab group with Mod+M and uses Ctrl+Shift+Space and Mod+Shift+1–9 for Screen navigation.

## Goals / Non-Goals

**Goals:** temporary group maximization without rewriting layout, stable terminal/session identity, correct hidden-pane activity and size claims, per-Screen restoration, and reference-compatible Screen shortcuts that do not steal editor or ordinary terminal input.

**Non-Goals:** floating/popout support, persistence, new server operations, general shortcut configuration, settings controls, browser changes or cross-platform release certification.

## Decisions

### Canonical group reference, not another layout

Store only an optional canonical slot ID in each `ScreenModel`. Derive the presentation root by borrowing that slot from the current snapshot, otherwise use the ordinary root. Expose this projection through the existing workspace API so rendering, visible-pane collection and focus validation use the same decision. Do not clone and mutate a second layout tree. The worker receives maximize/restore intent through its existing bounded command channel; it does not send a mux layout op for that intent.

### Reconciliation and user intent

Tab selection inside the maximized group remains available. New tabs retain maximization. A successfully admitted split or pane move restores the ordinary viewport to reveal its result; failed admission does not consume the view intent. Selecting/focusing a pane in another group restores the viewport. Snapshot reconciliation retains maximize state only while its canonical slot remains a nonempty slot; removal or replacement clears it and picks a valid visible focus. Each Screen remembers its own local maximize choice while another Screen is shown. New connection models start unmaximized.

Only the displayed group reports visibility and size. Other terminals remain attached, feed output and retain scrollback; restoring or returning to a Screen republishes measured sizes through the existing retry path. Closing or switching while maximized keeps the existing shared close assessment over the full affected scope, including hidden groups.

### Native presentation and shortcuts

Provide a visible maximize/restore control and Mod+M, preserving the web convention of maximizing a tab group rather than a single terminal removed from its tab strip. Keep drag/drop expressed as canonical pane moves, and restore on an admitted move instead of deriving a new layout from the maximized geometry.

Ctrl+Shift+Space creates a Screen even from the empty workspace. Mod+Shift+1–9 selects the corresponding entry in the current ordered local catalog, without generating input or another Screen. Extra modifiers are rejected and an unavailable index is a harmless consumed app gesture. Inspect the pinned GPUI key representation and verify actual shifted digit gestures; do not assume a US keyboard's shifted punctuation is the physical digit. Rename/editor focus and modal confirmation retain their existing key-routing priority.

## Architecture impact

Extend the existing `client_workspace` model in `architecture/client-core.c4` with a local viewport reference and projection. `native_workspace` owns the corresponding view intent alongside current local selection. `gpui_workspace` in `architecture/gpui.c4` renders that projection and translates controls/keys; no new component or dependency is needed. Update the local-workspace portion of `docs/architecture.md` and native usage/evidence in `docs/gpui-spike.md`.

## Risks / Trade-offs

- A surviving slot may lose its selected tab: reconcile focus against the same projected subtree, not against a stale pane ID.
- Hidden groups must not retain PTY size claims or lose output: exercise the actual worker and terminals, not just a visual screenshot.
- Maximized geometry differs from the canonical layout: use existing precise move ops and restore the viewport for moves/splits, never infer server split ratios from the maximized view.
- Shortcut behavior is platform-sensitive: unit checks cover modifier filtering; an isolated native-window check covers actual key delivery on the exercised Linux backend. Windows/macOS runtime acceptance remains open.

## Provisional decisions

- **NN-1 (2026-11-07, provisional, tbd):** maximize is a per-window, per-Screen canonical group reference. It neither persists nor changes the server layout; tab insertion retains it, admitted split/move or explicit focus outside restores it, and invalidated group identity clears it.
- **NN-2 (2026-11-07, provisional, tbd):** reuse the web Screen shortcuts and Mod+M in the native workspace. Editing and confirmation dialogs keep precedence, while the empty workspace still accepts the new-Screen gesture.
