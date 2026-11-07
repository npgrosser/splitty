# Tasks

## 1. Workspace shortcut dispatch

- [x] 1.1 Add a pure native shortcut decoder for the web-matching Cmd/Alt T, D, Shift+D, W and Shift+arrow chords; unit tests verify macOS and non-macOS modifiers, extra-modifier rejection, and ordinary Ctrl+C and Ctrl+Shift+C/V pass-through.
- [x] 1.2 Attach a capture-phase key handler to the native workspace root, dispatch recognized actions through its existing split, tab, close and focus paths, and consume each recognized event before terminal key handling; verify with the decoder unit tests and a real-window shortcut smoke in task 2.2.

## 2. Directional focus and integration

- [x] 2.1 Record rendered bounds for visible layout slots and choose the nearest slot center inside the requested 90-degree direction; focus its selected terminal without changing the authoritative tree. Unit tests cover a mixed split/tab layout, missing or stale bounds, tie-breaking, no neighbor, and the active terminal receiving focus.
- [x] 2.2 Extend the existing `--workspace-smoke` real-window path to dispatch keystrokes through GPUI's normal key event path with a focused terminal. Verify shortcut operations change the service-backed layout/focus, no shortcut produces session input, and a normal terminal key still does. A real-window smoke is needed because pure tests cannot prove capture dispatch runs before the terminal's bubble handler or that input stays out of the PTY route.
- [x] 2.3 Update the native workspace shortcut help in the executable/UI documentation and verify displayed chords match the platform decoder; architecture model and concept docs need no change because component boundaries remain unchanged.

## 3. Completion checks

- [x] 3.1 Run targeted `cargo test -p splitty-gpui --bin splitty-gpui` unit coverage and the existing `--workspace-smoke` on an available native graphics backend; record the result. Result: 24/24 unit tests and Wayland smoke passed; five shortcuts sent no terminal input and Ctrl+C reached the focused pane.
- [ ] 3.2 Run the full check once before merge: `cargo fmt --all --check`; `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`; `cargo test --workspace --exclude splitty-gui` with installed architecture dependencies; and `openspec validate --specs`.
