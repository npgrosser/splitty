# Tasks

## 1. Group drag in client-core

- [x] 1.1 Capture a group drag of a root-layout group and compute the `arrange_panes` of a drop beside a group, at the layout's edge or into a group, with each group's shown tab as its active tab; nothing for a drop on the group itself or one that changes nothing; a stale drag refused. Verify with unit tests in `workspace/group_move/tests.rs` (unit), including that the server's reducer accepts the arrangement without effects.
- [x] 1.2 Add `MoveGroup` to the native commands, admit it for the local source and in the host command gate, preview the move and drop the preview on `mux.op_failed` for `arrange_panes`. Verify with the host test `a_tab_group_of_a_host_screen_moves_by_drag_unless_its_daemon_is_older` in `native_workspace/remote/tests.rs` (in-process, a host's Screen through the hub client) and the preview rejection unit test `a_moved_group_shows_at_once_with_its_shown_tab_focused`.

## 2. Capability gate

- [x] 2.1 Add the command gate (`native_workspace/capability_gate.rs`): one table from command kind to capability, checked before a command reaches its server; a refused command settles its pending window state and yields one `ServerOutdated` status for the window, also for a host's Screen. Verify with the unit tests in `capability_gate/tests.rs` (unit: local and host message, passes with the capability) and the host test of 1.2 (in-process: nothing sent, layout unchanged).
- [x] 2.2 State the rule for later features in `docs/native-ui-handoff.md` and verify the section reads as the gate works.

## 3. Native view

- [x] 3.1 Make the empty tab bar area of tiled groups the group's grip, register the group drop on the existing drop targets and show the group's shown title with its tab count while dragging. Verify with the view tests `the_empty_tab_bar_area_drags_the_whole_group` and `a_group_dropped_on_an_edge_zone_goes_beside_that_group` in `workspace/transfer/view_tests.rs` (in-process view tests, `ui-test-support`).

## 4. Records and checks

- [x] 4.1 Add TGD-1 to TGD-5 to `openspec/open-decisions.md`.
- [x] 4.2 Run the targeted tests: `cargo test -p splitty-client-core --lib` with the filters `group_move`, `capability_gate` and `a_tab_group_of_a_host_screen` and `cargo test -p splitty-gpui --features ui-test-support -- transfer::view_tests`.
- [x] 4.3 Run the full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui` (needs `pnpm install --dir architecture` once), `cargo test -p splitty-gpui --features ui-test-support`, `openspec validate --specs`.
