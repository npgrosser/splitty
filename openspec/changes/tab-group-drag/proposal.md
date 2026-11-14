# Proposal

## Why

In the native app only single tabs can be dragged. Rearranging a group of several tabs takes one drag per tab, and the group's order and shown tab get lost on the way.

## What Changes

- The empty area of a tiled group's tab bar (right of its tabs and new-tab actions) drags the whole group. A tab is still dragged by itself; the area had no click, double click or menu before and has none now.
- The drop targets are those of a single tab: an edge zone of another group or of the whole layout puts the group beside it, the center of another group or a place in its tab bar puts all tabs of the dragged group into it, in their order, there or last, and the dragged group's shown tab becomes that group's shown tab. A drop on the group itself changes nothing.
- The window sends the result as one `arrange_panes` (`layout-arrange-v1`): the root layout as the window shows it, with the group moved. No server, op or protocol change.
- Local Screens and a host's Screens alike.
- A command layer gate in the native client: a command that needs a `mux.welcome` capability is looked up in one table; a server that did not announce it gets nothing, and the window shows one status message that the server (for a host, its daemon) is older than the app and needs an update. The group move is its first row (`layout-arrange-v1`).

Out of scope: dragging floating groups (their header moves the floating group), moving a group to another Screen (a change of its own), a keyboard equivalent, and any change to the web client.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workspace-model`: the native tab group move (grip, drop targets, result, staleness).
- `mux-protocol`: native windows send `arrange_panes` for a whole group's move; commands that need a capability the server did not announce are not sent.

## Impact

`crates/splitty-client-core` (the group drag and its arrangement in `workspace/group_move.rs`, the command kind, the command gate in `native_workspace/capability_gate.rs`, the window status for a refused command), `crates/splitty-gpui` (the grip, the group drop on the existing drop targets). The server is unchanged: `arrange_panes` already exists and every released server announces `layout-arrange-v1`.

## Architecture impact

None: new submodules inside the existing client-core and GPUI components, no new crate, top-level module or dependency between components.

## Decisions

- **TGD-1** (provisional, tbd): the result is `arrange_panes` with the whole root layout, since it says the outcome in one atomic op with no new server code. A layout another client changed in between makes the drag stale before the drop, and the server rejects an arrangement that misses a pane added meanwhile. Alternative: a dedicated `move_group` op that names only source and target.
- **TGD-2** (provisional, tbd): the grip is only the empty area of a tiled group's tab bar; floating groups are not dragged as groups, and the "lock tabs" setting locks group drags too. Alternative: a dedicated grip icon in every tab bar.
- **TGD-3** (provisional, tbd): a group dropped beside another one splits that group's place in halves, as a single tab dropped there does; dropped into another group its tabs go before the marked tab or last, and its shown tab is shown there. Alternative: keep the dragged group's own size.
- **TGD-4** (provisional, tbd): since `arrange_panes` gives every node a new ID, the arrangement names each group's tab shown in this window as its active tab, so that no group of this window changes its shown tab with the move. Other windows then show those tabs as well.
- **TGD-5** (provisional, tbd): commands that need a newer server are gated in the native command layer by the server's `mux.welcome` capabilities, not hidden in the views: the action is offered, and an older server gets nothing while the window says it needs an update. Never a version comparison. Existing checks that change what a command does (a close without `close-assessment-v1` asks generically) or keep a drop target from being offered (`tab-placement-v1`) stay where they are. Alternative: hide or disable the action per capability in the views.
