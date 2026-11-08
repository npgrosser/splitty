# Tasks

## 1. Shared source admission and dispatch

- [x] 1.1 Add reusable side-effect-free rename availability to the existing source workspace, preserving local behavior and requiring current canonical presence for remote Screens; verify with unit tests extending source workspace fixtures for ready, missing, unconfirmed, removed and disconnected targets.
- [x] 1.2 Permit the existing captured RenameScreen command through the remote runtime without reconnect or replay; extend in-process remote/source dispatch tests with colliding Screen IDs and stale connection/close gates to prove only the intended current host receives the operation.
- [x] 1.3 Document shared admission and exact remote routing in the existing client_workspace/native_workspace components and relationships in architecture/client-core.c4; verify the architecture check and the unchanged rename_screen wire shape against docs/protocol.ts.

## 2. Captured inline rename editor

- [x] 2.1 Capture window owner, source token and ScreenKey in the existing rename editor and revalidate its captured source on activation, reconciliation and commit; extend GPUI in-process production-view tests to preserve input/focus through unrelated selection/output updates and reject replaced, removed, missing, unconfirmed or closing targets without replay.
- [x] 2.2 Enable the same Drawer input and double-click/F2/context entrypoints for eligible remote rows while preserving local entrypoints; extend the existing GPUI in-process Drawer interaction test with real pointer/keyboard input, colliding remote/local Screen IDs, Enter/Blur commit and Escape cancellation, verifying exact source dispatch and unchanged selection.
- [x] 2.3 Preserve canonical name adoption and existing bounded validation/refusal feedback; extend in-process tests for invalid names, full command admission, canonical refresh and connection loss to prove no optimistic rename or duplicate/replayed operation.
- [x] 2.4 Update the existing GPUI workspace architecture description/relationships and docs/architecture.md editor identity account; verify they describe captured source ownership and canonical names without introducing another model or dependency.

## 3. Integration verification

- [ ] 3.1 Extend the existing owned remote-screen window smoke fixture with an actual remote rename and canonical observation, including another source selected and normal cleanup; verify exact host naming and preserved selection/sessions. This existing real-process/window check covers platform input/focus and end-to-end catalog observation that in-process fixtures cannot establish.
- [x] 3.2 Run targeted client-core and GPUI tests, strict Clippy for touched crates, formatting, architecture tests and strict OpenSpec change validation; verify all pass and record exact source/evidence in docs/gpui-spike.md.
- [ ] 3.3 At the deferred pre-merge checkpoint, run the one full check from CLAUDE.md: cargo fmt --all --check; cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings; cargo test --workspace --exclude splitty-gui; openspec validate --specs. Verify the architecture dependencies are installed before the architecture check. Full workspace CI remains deferred during native MVP iteration.
