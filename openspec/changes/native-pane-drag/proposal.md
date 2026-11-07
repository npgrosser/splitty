# Proposal

## Why

The native local workspace can create tabs and splits but cannot relocate an existing terminal. Single-tab dragging completes a useful arrangement workflow without restarting sessions, advancing [Native UI with GPUI](../native-ui-gpui/proposal.md) from roadmap item 120.

## What Changes

- Move one terminal tab into another visible slot or onto any of its four edges through the existing `move_pane` operation.
- Preserve pane/session identity, terminal state and surviving source proportions; change local selection only after authoritative placement.
- Cancel stale gestures and surface admission or server rejection without changing the displayed layout optimistically.
- Keep group moves, arbitrary tab ordering, floating panes, cross-screen/window moves, and external dragging out of scope.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workspace-model`: require native pane gestures to retain session identity, reject stale captured routes and await authoritative placement before changing selection.

## Impact

Existing client-core workspace, GPUI workspace and worker modules; focused tests and architecture documentation. No wire format, server reducer, dependency or released web-client changes. The development Windows launcher remains outside scope.
