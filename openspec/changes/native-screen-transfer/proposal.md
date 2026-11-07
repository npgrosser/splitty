# Proposal

## Why

The native workspace can organize tabs inside one Screen, but cannot move a running terminal to another Screen. Its current per-Screen reconciliation can also detach a live terminal when another client performs that already supported server operation.

## What Changes

- Add a native tab action menu for transferring a captured terminal tab to an existing or new Screen on the same service.
- Reconcile cross-Screen route changes coherently, preserving the terminal session, attachment and retained view state for both local and externally initiated transfers.
- Submit each transfer once, verify its authoritative result, and follow the transferred tab only when later local navigation has not superseded that intent.
- Keep client admission, reconciliation and transfer results in the UI-independent client layer; retain the existing server reducer as the workspace authority.

Out of scope: cross-service transfers, pane renaming, multi-tab close commands, floating layouts, templates, replacement of the web client, a WASM port, release integration and full merge CI.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workspace-model`: native cross-Screen transfer admission, coherent observation, session retention and local navigation semantics.

## Impact

Touches the native client workspace/runtime, native tab UI, targeted tests and architecture documentation. Reuses `move_pane_to_screen`, Screen snapshots and the ordered workspace catalog query without changing wire formats or server effects. The web implementation remains available as a reference and client. This increment makes no decision about eventual native/web replacement or coexistence.
