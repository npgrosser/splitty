# Proposal

## Why

The native workspace attaches floating terminals but only renders and sizes the tiled root. Projecting the server's complete layout makes those terminals usable, adds the existing float/dock workflow and removes a prerequisite for faithful template support.

## What Changes

- Render authoritative floating groups alongside tiled panes, retaining terminal entities and consistent tab, focus, maximization and visible-size behavior.
- Add captured float/dock actions and moving/resizing a floating group through existing server operations. Local gesture previews and stacking remain presentation state.
- Support tabs, close, shell-profile choice and cross-Screen transfer from floating groups; keep the protocol's root-only split, divider and tab-placement operations explicitly unavailable there.
- Validate floating bounds consistently with template application in the shared reducer boundary, and fix removal of a pane from a nested floating tree so surviving siblings and sessions remain intact.
- Verify overlapping pointer targets, stale/cancelled gestures, viewport/zoom changes, external layout updates and real terminal state retention.

Out of scope: new floating split or cross-surface tab-placement protocol semantics, popout OS windows, templates themselves, production workspace migration, remote UI, web removal, packaging and full merge CI.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workspace-model`: native floating projection and gestures, valid shared floating bounds, and regression coverage for closing inside a floating subtree.

## Impact

Touches shared mux validation/reduction, UI-independent native workspace projection/runtime, GPUI floating presentation and targeted test tooling. Reuses `float_pane`, `dock_pane`, `move_float` and existing tab/session operations. Bounds validation preserves finite offscreen coordinates and positive oversized dimensions already used by web clients. No wire shape or library dependency changes are planned. Continues the [native UI evaluation](../native-ui-gpui/proposal.md) without deciding web/native coexistence or cutover.
