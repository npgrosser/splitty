# Tasks

## 1. Startup phases and the prepared WSL target

- [x] 1.1 Add the startup phases, the WSL preparation on its own thread (phases, result, stop flag, joined on stop) and an optional resolved SSH dialog path to the WSL launch target in client-core; unit tests in `owned_service` cover phases then target, a failure as error, stop joins the thread, drop signals stop, and the dialog path validation
- [x] 1.2 Let the workspace worker wait for a preparing target, forward its phases, report starting the backend and connecting, and report a failed start as a local service failure, and treat a stop during the preparation as no failure; in-process tests (`native_workspace::startup_tests`) show a failed preparation's phase and then its reason, and a stop that joins the preparation, starts nothing and reports no error
- [x] 1.3 Close the window at once while the WSL target is prepared (the worker reads no close request yet); unit test of the preparing phases in `startup_page`

## 2. Windows preparation

- [x] 2.1 Run the preparation from `main` before the window opens; report its phases, make every `wsl.exe` call stop on the stop flag, record the distribution when it succeeds, and keep the remaining dialogs topmost; checked by a cross-target `cargo check --target x86_64-pc-windows-msvc` with a stand-in resource compiler (Windows-only code is not run here)
- [x] 2.2 Translate the SSH dialog's Windows path in the inspection call and skip the separate `wslpath` call when it is known; unit tests run the inspection script with a stand-in `wslpath` (translated, empty, failing) and check the launch target keeps only an absolute path

## 3. Window

- [x] 3.1 Show the startup page (phase or failure with its phase) until the workspace arrives, sharing the empty page's column; a failure after the workspace arrived stays a status line; unit tests of the phase texts and the failure state, view tests (`--features ui-test-support`) for the page, the failure page and the status line
- [x] 3.2 Load the settings' WSL section once the prepared distribution is known
- [x] 3.3 Record the startup behavior in `docs/native-ui-handoff.md` and the provisional decisions NSP-1 to NSP-3 in `openspec/open-decisions.md`; update the descriptions of `owned_service`, `gpui_main` and `gpui_startup` in `architecture/`

## 4. Checks

- [x] 4.1 Targeted tests: `cargo test -p splitty-client-core`, `cargo test -p splitty-gpui --features ui-test-support`
- [x] 4.2 Before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support`, `openspec validate --specs` and `openspec validate native-startup-progress`
