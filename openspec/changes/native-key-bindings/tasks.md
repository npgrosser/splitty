# Tasks

## 1. Registry, keymap and dispatch

- [x] 1.1 `splitty_client_core::keymap`: commands, chords, defaults per platform, overrides file, effective keymap with conflicts and problems; architecture model updated
- [x] 1.2 Unit tests: chord parsing and labels, every default, no shared default chord, override semantics, file round trip and problems (scenarios "Replaced and removed keys", "Chord taken from another command", "Problems in the file")
- [x] 1.3 The window looks keys up in the keymap; find, copy and paste of the terminal too; one command dispatch for keys and the palette
- [x] 1.4 Every key label comes from the keymap; `--help` is generated from the registry
- [x] 1.5 In-process tests: a default key runs its command, an override rebinds it, an unbound command shows no label, Ctrl+B and Ctrl+F reach the terminal (scenarios "Rebound key in a menu", "Unbound command", "Keys left to terminal programs"; extends the workspace's shortcut tests)

## 2. Commands that had no key

- [x] 2.1 Screens: next and previous, close, rename, move up and down
- [x] 2.2 Tabs and panes: new tab with profile, close other tabs, move tab left and right, move to another Screen, float or dock, focus the drawer
- [x] 2.3 Terminal and window: clear buffer, history scrolling on the normal screen, copy working directory, open selection in the viewer, full screen, quit
- [x] 2.4 In-process tests per command group (scenarios "Next Screen", "History keys on the alternate screen")

## 3. Arrange mode

- [x] 3.1 Mode state, resize by arrows, move by Shift with arrows, end by Escape or Enter and by leaving the Screen; the window says the mode is on
- [x] 3.2 In-process tests (scenarios "Resize from the keyboard", "Move into the neighboring group", "End of the mode")

## 4. Keyboard page

- [x] 4.1 Settings page: commands by category with search, record, remove, reset, problems and notes; saves and applies at once; the palette's entry opens it
- [x] 4.2 In-process tests (scenarios "Record a key", "Reset")

## 5. Docs

- [x] 5.1 `docs/keyboard.md` with the default table, checked against the registry by a unit test; README table replaced by it; `openspec/open-decisions.md`: NKB-1 to NKB-7

## 6. Checks

- [x] 6.1 Targeted: `cargo test -p splitty-client-core`, `cargo test -p splitty-gpui --features ui-test-support`
- [x] 6.2 Full check before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `openspec validate --specs`
