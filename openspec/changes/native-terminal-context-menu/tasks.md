# Tasks

## 1. Terminal menu interaction

- [x] 1.1 Emit local context intent from the admitted terminal pointer route and share Copy/Paste/Find implementations with keyboard input; verify application-reporting priority, Shift gesture lifetime and exact paste framing through a production-view in-process test.
- [x] 1.2 Compose one captured Kit popup in Workspace and reuse existing focus, split, Drawer reveal and assessed-close commands; extend the existing workspace production-view fixture to exercise real menu pointer/keyboard activation on an initially unfocused displayed split, dismissal, and unchanged command targets.
- [x] 1.3 Revoke stale menu authority after source, Screen, pane/session or terminal replacement; verify in-process that obsolete actions send no input, split or close command and never focus a replacement terminal.
- [x] 1.4 Extend existing Focus with exact retained terminal identity and handle it as worker-owned model intent before transport routing; verify offline focus survives unrelated publication and stale owner/session, hidden pane, closing and disposed targets fail in unit tests, plus real worker source-envelope rejection and production-view offline Copy/Find.
- [x] 1.5 Update existing terminal/workspace descriptions and relationship in `architecture/gpui.c4`, shared model and worker descriptions in `architecture/client-core.c4` and the native interaction section of `docs/architecture.md`; verify the architecture model checks pass without a new component or dependency.

## 2. Integration checks

- [x] 2.1 Run focused terminal, workspace menu and client-core focus/source tests, strict GPUI and client-core all-target Clippy and `openspec validate native-terminal-context-menu --strict`; verify each passes and record scope.
- [ ] 2.2 At the later merge checkpoint, run `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui` and `openspec validate --specs`; install architecture dependencies with `pnpm install --dir architecture` first if absent. Full CI remains deferred until that checkpoint.

Targeted verification: workspace production/unit tests 93 passed (including five terminal-menu tests and 15 obsolete-action cases); terminal tests 17 passed; shared offline-focus unit test, four source-worker tests and two navigation-worker tests passed. Strict GPUI/client-core all-target Clippy, architecture checks (9 unit and 8 model), formatting and strict change validation passed. Offline publication retention is covered at the shared model unit boundary; source-envelope rejection and normal focus dispatch additionally run through the actual native worker. No new real-process harness or full merge CI was run.
