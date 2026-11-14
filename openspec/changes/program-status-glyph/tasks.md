## 1. Status glyph

- [x] 1.1 `workspace/program_status.rs`: one function that gives a pane's icon-place content from its shown record and its title's leading icon (state glyph, idle glyph, title icon or none), with its color; unit tests for each state, for a seen `done` record and for an idle record with and without a title icon
- [x] 1.2 Draw it in the drawer's pane row in place of the leading icon and the dot, animated for `working`
- [x] 1.3 Draw it in the tab label and in the passive tab drawings; remove the dot behind the tab title
- [x] 1.4 View tests (in-process, `ui-test-support`): extend the tab status and drawer title icon tests to the glyph in both places and to the idle rule

## 2. Checks

- [x] 2.1 `cargo test -p splitty-gpui --features ui-test-support`, clippy, fmt, `openspec validate --specs`
- [ ] 2.2 Look at a working, a blocked and an idle program in a built app
