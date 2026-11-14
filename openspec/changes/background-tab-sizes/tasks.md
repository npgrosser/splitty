# Tasks

## 1. Background tabs follow their group's size

- [x] 1.1 Give the layout model the slot of a pane and the tabs of a displayed slot with the tab the window shows there; verify with the tests of 1.3
- [x] 1.2 In the native client's size reporting, remember the tab groups of the shown panes, report their other terminal tabs at the shown pane's measured size and again when it changes, keep a reported tab while its group's size is not measured yet, hide them once the group is no longer shown, leave groups whose shown tab is not a terminal and servers without `pty-size-v1` as before; verify with the tests of 1.3
- [x] 1.3 In-process tests in client-core next to the existing size report tests (a remote source over the test service, PTY sizes read with `stty size`): a background tab takes the shown tab's size and follows a new one; a tab switch withdraws no report (no other client's size shows in between); maximizing another slot and showing a Screen of another source hide both sessions; a server without `pty-size-v1` leaves the background tab's size alone. Verify the main test fails without 1.2
- [x] 1.4 Note the behavior in the PTY size section of `docs/native-ui-handoff.md` and add BTS-1 to BTS-3 to `openspec/open-decisions.md`

## 2. Checks

- [x] 2.1 Targeted tests: `cargo test -p splitty-client-core`
- [x] 2.2 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support` and `openspec validate --specs`
