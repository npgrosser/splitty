# Tasks

## 1. Owned local settings model

- [ ] 1.1 Extend `splitty-client-core` local presentation with bounded effective settings, metadata and theme-list reads on its existing owned API stream; verify in-process tests in `appearance/local/tests.rs` cover local source while a remote Screen is selected, valid choices/defaults, and missing metadata/catalog without blocking terminal delivery.
- [ ] 1.2 Add typed single-field `home.update` intents with connection/request IDs, a 16-intent FIFO, one active request and at most one accepted intent per field; verify in-process tests in `local_service/tests.rs` cover successful reply and same-service `settings.changed`, remote-source rejection, duplicate/queue rejection, and close-time discard/cancellation with no replay.
- [ ] 1.3 Reconcile rejected, uncertain, stale and superseded writes through a fresh local `home.get` without automatic replay; verify in-process tests in `local_service/tests.rs` cover each outcome, field error visibility and last-confirmed presentation.

## 2. Independent palette and theme projection

- [ ] 2.1 Resolve `terminal.theme` from `theme.list`/`theme.get` on the local stream: `default` inherits UI theme, named themes supply only terminal tokens, mismatched/fallback names surface errors; verify in-process tests in `appearance/local/tests.rs` cover separate choice, auto-mode inheritance, unavailable palette and superseded palette reads.
- [ ] 2.2 Apply palette changes to existing and new GPUI terminal views without resetting engine, history or font metrics from a palette-only change; verify unit tests in `crates/splitty-gpui/src/appearance.rs` and in-process workspace tests cover the same terminal identity and preserved scrollback across palette switches.
- [ ] 2.3 Project supported service UI colors to GPUI Kit 0.9 semantic roles after mode changes, retaining bounded fallbacks; verify unit tests with light, dark, custom and invalid resolved tokens, plus in-process GPUI UI integration checks for readable control states and no stale mode repaint.
- [ ] 2.4 Update `architecture/client-core.c4`, `architecture/gpui.c4` and the **Settings authority**/native appearance sections of `docs/architecture.md` for the local write owner and separate palette; verify the architecture model check in `cargo test -p splitty-architecture` passes.

## 3. Native Settings presentation

- [ ] 3.1 Build a Settings destination with Appearance, Terminal and Workspace groups using controlled GPUI Kit components and stable focus IDs; verify in-process GPUI UI integration tests exercise keyboard/pointer navigation, Escape focus return, scroll ownership, and preserved selected Screen/session on open and close.
- [ ] 3.2 Bind the twelve supported fields to confirmed local values and server metadata/catalog; show pending, disabled, reset, rejection and uncertainty states without write-on-open; verify in-process GPUI UI integration tests cover each group, a successful reset, UI font family and size selection, missing metadata/catalog, and a rejected update.
- [ ] 3.3 Add a short native real-window smoke path to the existing GPUI testing scripts for saved theme/palette, visible Settings access, and terminal survival after edits; a headless in-process test cannot establish actual window theme/geometry or terminal rendering. Verify the smoke on an optimized Linux window and record Windows/macOS platform evidence separately where available.
- [ ] 3.4 Update the native development runbook in `docs/gpui-spike.md` with the supported controls, local-service ownership and unsupported settings sections; verify its descriptions match the implemented editor.

## 4. Integration gate

- [ ] 4.1 Run targeted `cargo test -p splitty-client-core` and `cargo test -p splitty-gpui`, the selected GPUI UI integration tests, and the short real-window check; verify all newly changed behaviors pass.
- [ ] 4.2 Run `cargo fmt --all --check`, targeted `cargo clippy -p splitty-client-core -p splitty-gpui --all-targets -- -D warnings`, `cargo test -p splitty-architecture`, and `openspec validate native-settings --strict` plus `openspec validate --specs`; verify every targeted gate passes.

The full workspace Clippy/test and other main-merge checks remain obligations after the native MVP, not a gate for this increment.
