# Tasks

## 1. Server

- [x] 1.1 Add the workspace op `move_tab_group_to_screen` and its reducer: every check before any change, the panes out of the source as `close_pane` takes them, into the target's first slot in tab order with the shown tab active, a created target after the source, an emptied source removed (TGT-1, TGT-2, TGT-3)
- [x] 1.2 Decode the op on the WebSocket (malformed: ignored; rejected: `mux.op_failed`) and on the control API (malformed: 400; accepted: `cli_op` nudges for both Screens) (TGT-4)
- [x] 1.3 Announce `tab-group-transfer-v1` last among the session capabilities of `mux.welcome`
- [x] 1.4 Unit tests (reducer): a group to an existing Screen in tab order with its sessions and no effect, the group's active tab without `activePaneId`, a new Screen after the source, the last group taking its Screen along, a floating group into the target's grid, every rejection leaving the workspace unchanged, a job's pane refusing the group, an empty slot
- [x] 1.5 In-process tests: the welcome's capability order; an accepted move nudging the others for both Screens and not the sender; a rejected one answering only its sender; a malformed one ignored; the control API move, its 400 and its rejection

## 2. Native client

- [x] 2.1 Capture a tab's group (slot, tabs with sessions, the shown tab), validate it again before sending, and send it once as a transfer whose pending state lists every moved pane and the op that rejects it; verify every pane on the target before following to the shown tab (TGT-5)
- [x] 2.2 A `TransferGroup` command for the local source and hosts, with its row `tab-group-transfer-v1` in the command gate; a refused command settles the transfer as rejected before the status message
- [x] 2.3 "Move Group to Screen" in the tab menu for groups of more than one tab, with the Screen choices of "Move to Screen", beside its own row, by pointer and keyboard
- [x] 2.4 Unit tests: capture from any tab with the shown tab, a floating group, a changed group and a group with a job's pane refused, one send and only its own `mux.op_failed` rejecting it; the gate row for a local service and a host's daemon
- [x] 2.5 In-process tests: a local worker with live PTYs moving a group whose target went away behind its back (rejected, group unchanged) and then to a new Screen (confirmed, processes unchanged, emptied source gone, the shown tab followed); a host's Screen refused while its daemon lacks the capability and moved once it has it; the view's entry, its submenu, the command it sends, the confirmation and both refusals as status

## 3. Docs

- [x] 3.1 `docs/protocol.ts`: `MoveTabGroupToScreenOp`, the op union and the welcome's capability list
- [x] 3.2 `docs/architecture.md`: the op beside `move_pane_to_screen` and in the cross-Screen transfer paragraph
- [x] 3.3 Keep `move_tab_group_to_screen` in the op list of the pending `native-screen-reorder` delta
- [x] 3.4 Bullets for TGT-1 to TGT-5 in `openspec/open-decisions.md`

## 4. Checks

- [x] 4.1 Targeted tests: `cargo test -p splitty-core mux::reducer`, `cargo test -p splitty-server mux_handler move_pane`, `cargo test -p splitty-client-core transfer capability_gate remote::tests`, `cargo test -p splitty-gpui --features ui-test-support workspace::transfer`
- [x] 4.2 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support`, `openspec validate --specs`
