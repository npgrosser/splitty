# Design

## Context

See proposal.md for motivation. The server already implements `move_pane_to_screen` without PTY effects. It emits invalidations for both affected Screens; an emptied source disappears from the catalog. Native `LocalWorkspace` currently reconciles each snapshot independently. Source-first delivery removes a route before its new owner arrives; target-first delivery conflicts with global pane/session uniqueness. Catalog pruning can remove the source first as well. GPUI retains terminal entities by those routes, so fixing only the menu would lose retained state.

## Goals / Non-Goals

Maintain one canonical committed workspace and a bounded temporary collection of authoritative observations. Keep session delivery and retained terminal identity independent of Screen membership. Reuse current wire messages and server semantics. Do not create a second optimistic layout, a generic transaction framework, or a browser migration layer.

## Decisions

### Capture the action's actual source

`PaneActionTarget` captures connection, Screen, pane and session identity. Capture and worker admission accept a non-selected terminal tab in the currently shown canonical root. Admission rechecks ownership, exact route, root membership, managed-job restrictions, target existence/collision, capacity and conflicting pending operations. It does not retarget to whichever pane gained focus while a menu was open. An existing target is one Screen on the same service; a new target gets one client-generated ID and omits `screenName` so server naming remains authoritative.

One identified transfer is submitted once. A successful queue admission is not a server acknowledgment. A typed result distinguishes confirmed placement, rejection and an unverified outcome. No automatic mutation retry is allowed.

### Commit route ownership coherently

Local transfers enter a workspace refresh batch immediately after admission. An ordinary snapshot that removes an established route, claims another Screen's route, or catalog pruning that would remove live routes enters the same path, covering external moves. Metadata and snapshots that do not change route ownership retain the fast path.

Keep committed models/routes available while collecting. Use the existing ordered workspace catalog request as an opening fence, request each listed Screen snapshot once, then enqueue a trailing catalog request after those requests were admitted. Missing Screen resyncs have no response, so the trailing catalog is essential to close the collection without guessing timeouts. The serial connection proves that preceding read requests were handled; it does not promise a globally atomic snapshot against other clients.

Validate the complete actual candidate before publishing one global route delta. Enforce existing Screen, attachment and snapshot limits while collecting as well as on commit; do not retain unbounded JSON or bypass uniqueness. Carry readiness/attachment facts across ownership by stable pane/session identity. Removed Screens may be omitted only when the ending catalog confirms their removal. Membership changes can rearm a bounded read-only collection; inconsistent or malformed candidates retain committed state, release staged memory and expose verification failure. Subsequent invalidations or explicit refresh can retry reads, never the transfer mutation. Avoid unbounded immediate recollection under concurrent churn.

### Preserve local choices deliberately

Once the transfer is verified, follow its target and select the moved pane unless later local navigation canceled that deferred focus intent. External transfers do not create follow intents. Retain still-valid local selections and maximized slots; reveal the moved pane by restoring the target's full layout only if its current maximized slot would hide it. An emptied source follows normal catalog fallback when no still-valid deferred follow exists.

While routes are unsettled, reject conflicting pane/Screen mutations and close intents. Window close remains available with unresolved reconciliation included in its assessed impact, preserving the existing explicit confirmation and joined service shutdown path. Terminal output continues throughout collection.

### Keep the first menu narrow

The tab menu exposes transfer destinations and New Screen, through a visible tab affordance and tab right-click. It captures its source without first selecting it. Opening/dismissing a menu sends no operation. Terminal-body right-click remains available to terminal applications. Existing tab close and creation controls remain; multi-close and unrelated menu actions are separate increments.

## Architecture impact

Update `splitty.client_core.client_workspace` and `splitty.client_core.native_workspace` descriptions in `architecture/client-core.c4`, and `splitty.gpui.gpui_workspace` in `architecture/gpui.c4`. Existing workspace-to-core model/client relationships and GPUI-to-client workspace/runtime relationships suffice; no new top-level component or dependency is needed. Extend the Client connections section of docs/architecture.md with coherent cross-Screen ownership and transfer settlement. The server remains the authority for domain state and effects; the client core owns observation/admission and local navigation; GPUI owns menu presentation and retained terminal entities.

## Risks / Trade-offs

- Concurrent external changes can prevent a consistent read collection → retain committed routes, report unverified state and permit bounded read recovery without replaying mutations.
- A complete collection temporarily duplicates model data → apply existing aggregate caps during collection and release failed candidates.
- Following a delayed result can steal focus → cancel only the follow intent on later local navigation, retaining verification.
- New UI behavior is platform-sensitive → verify real private Linux windows and state the limits; Windows/macOS acceptance follows separately.

## Provisional decisions

- **NST-1** (2026-11-07, provisional, tbd): use existing ordered catalog/snapshot reads for bounded coherent reconciliation. A future atomic workspace snapshot protocol remains an alternative if measured churn or cost warrants it.
- **NST-2** (2026-11-07, provisional, tbd): follow a locally transferred tab after confirmation unless later navigation intervenes; keep the first tab menu limited to Screen transfer.

## Migration Plan

This is additive on the experimental native feature branch. Preserve the current frozen preview until the new release binary passes targeted and actual-window checks. Rollback uses that retained preview or reverts this increment; no persisted data or wire migration is involved. Web/native coexistence and any eventual cutover remain open.
