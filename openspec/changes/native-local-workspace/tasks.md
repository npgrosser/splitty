# Tasks

## 1. Shared local workspace client

- [x] 1.1 Add typed screen operations and the authoritative local workspace model with multi-session routing, pending pane matching and window-local selection; verify real-service in-process split/tab/select/resize/close tests, capability-gated tab persistence and close-final-pane/refill of the same screen.
- [x] 1.2 Extend in-process tests for unchanged pane identity, removal/replacement and generation-scoped input/output; keep core independent of UI/emulator.

## 2. Native workspace presentation

- [x] 2.1 Render the shared tree with terminal entities keyed by pane ID, tab bars and native split dividers; verify split, tab creation/switching, resizing and close in the native local-workspace smoke.
- [x] 2.2 Make the local workspace the default development window, retain the legacy probe explicitly, and preserve joined isolated cleanup; verify both launch modes and service teardown.
- [x] 2.3 Update the architecture model and native development documentation; verify model checks and documented launch commands.

## 3. Acceptance and completion

- [x] 3.1 Run focused client-core in-process and native presentation tests plus actual-window workspace smoke on Wayland/X11. Extend the real-window harness because in-process reducer/client tests cannot verify focus, clipping, native input and divider/tab interaction.
- [ ] 3.2 Run the full check after the implementation is stable: cargo fmt --all --check; cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings; cargo test --workspace --exclude splitty-gui including architecture model dependencies; openspec validate --specs.
- [ ] 3.3 Commit, sync the client-connections requirement and archive the completed increment; update the parent migration/roadmap with implemented scope and remaining work.
