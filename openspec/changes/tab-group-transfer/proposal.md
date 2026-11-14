# Proposal

## Why

A tab group moves to another Screen today one tab at a time: one `move_pane_to_screen` per tab, each a change of its own that other windows see, with the group's order and shown tab rebuilt by hand on the target. A group of related terminals should move as one.

## What Changes

- A new workspace op `move_tab_group_to_screen` moves every pane of one slot (a tab group of the root layout or of a floating entry) with its session to another Screen of the same server, or to a new one right after the source, in one reduction: the tabs keep their order and the named tab (else the group's active one) is shown on the target. No session starts or ends. A source left without panes is removed with the op. A group holding a managed job's pane is refused as a whole.
- Servers announce it with the `mux.welcome` capability `tab-group-transfer-v1`. Accepted, it nudges the other clients for both Screens; rejected, its sender gets `mux.op_failed`. The control API takes it as a workspace op with the same nudges.
- The native tab menu gets "Move Group to Screen" for a group of more than one tab, with the same choices as "Move to Screen" (the other Screens of the tab's server, then a new Screen). The window sends one `move_tab_group_to_screen` and follows to the group's shown tab once the move is verified, as after a single tab's move.
- The entry is offered whatever the server. The command layer gate of the native client gets one row: without `tab-group-transfer-v1` nothing is sent and the window says the server (for a host, the host's daemon) needs an update. A refusal the server sends back is an ordinary status.
- `docs/protocol.ts` and `docs/architecture.md` describe the op.

Out of scope: dragging a group onto another Screen's tab in the Screen list, a group move in the web client (its wire types stay as they are; no check compares them), keeping the moved group as a group of its own beside the target's groups, and a keyboard shortcut.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workspace-model`: the new layout op, its rules and rejections, and the native group transfer.
- `mux-protocol`: the `tab-group-transfer-v1` capability, its propagation and rejection; the native command gate covers the group transfer.
- `control-api`: `move_tab_group_to_screen` as a workspace op with nudges for both Screens.

## Impact

`crates/splitty-core` (the op, its reducer and errors, the capability), `crates/splitty-server` (decoding on the WebSocket and the control API), `crates/splitty-client-core` (the group's capture and validation, the transfer's pending state and verification, the command and its gate row), `crates/splitty-gpui` (the tab menu entry and its submenu). Older servers ignore the unknown op without an answer, so the client gate is what keeps a window from waiting on them.
