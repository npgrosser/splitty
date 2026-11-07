# Design

## Context

The server already supports atomic `MovePane`, with no PTY effects, and the native workspace retains terminal entities by pane and session. Window-local selected tabs override the snapshot's active tabs. GPUI provides typed internal drag events and cancellation. See proposal.md for scope.

## Goals / Non-Goals

Keep service snapshots authoritative during gestures and keep input routes, focus and visibility coherent after placement. No optimistic dock tree, group payloads, external drag payloads or new dependency.

## Decisions

- A UI-free captured move contains connection ID, screen ID/revision, source slot, pane/session and a target slot/placement. Revalidate at UI drop and worker admission. Compare the captured immutable canonical root structure with the current root, including node IDs, pane membership/order, axes and sizes. Metadata-only revisions and active-tab changes remain valid. The captured snapshot is an Arc to the existing snapshot, not another mutable layout.
- Use one `MovePane`: center appends to a different slot; edges wrap a target leaf slot with a 50/50 split. Same-slot center is a no-op; same-slot edge requires another tab. Never target an ancestor that removal can collapse.
- Normalize source sibling sizes after removing an emptied source slot. Generate only finite valid sizes. The server operation remains unchanged.
- Track one pending placement separately from pending creation focus. Confirm a newer snapshot's target slot or new adjacent split before selecting the moved pane. Earlier snapshots cannot consume that intent; failures and route reset clear it. A subsequent user focus choice cancels deferred focus.
- Drag only the tab label, with a bounded visual preview. One slot drop handler uses current bounds to resolve a center or edge zone. Escape cancels the active drag. Hover changes decoration only; successful drops enter the shared bounded worker queue once.

## Architecture impact

Update descriptions of `splitty.gpui.gpui_workspace`, `workspace_session` and `splitty.client_core.client_workspace` in architecture/gpui.c4 and architecture/client-core.c4. Existing relationships suffice. Extend the Client connections concept in docs/architecture.md with captured move admission and snapshot-confirmed focus. No new top-level module or protocol type.

## Risks / Trade-offs

- Structural snapshot changes invalidate a gesture: show a concise rejection and require a new gesture.
- Existing local tab selection can hide a moved pane: confirm placement before replacing that destination's selection.
- Stale bounds after layout replacement: require a current root slot and current rendered target bounds; cancel on structural change.
- No request ID or revision precondition exists on `move_pane`: local guards cannot prevent a concurrent server edit after send; atomic server validation and snapshot reconciliation remain authoritative.

## Migration Plan

Development-native behavior only; existing clients and wire formats remain unchanged. Reverting the UI/client changes restores the previous behavior without persisted-data migration.

## Validation boundary

This increment uses targeted client-core/GPUI checks, architecture consistency
and actual native pointer interactions as its MVP gate. Broad workspace/CI and
cross-platform release validation remain part of later cutover work; this change
does not claim those platforms or a released-client migration are verified.
