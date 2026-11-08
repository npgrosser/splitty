# Tasks

## 1. Shared creation admission and existing wire operation

- [ ] 1.1 Add a narrow Core creation capture/validation reader and explicit remote creation command carrying request identity, existing host action capture and remote execution envelope. Keep local CreateScreen semantics unchanged. Extend existing `workspace/window/tests.rs` and `native_workspace/source_tests.rs` unit/in-process tests for wrong local owner, old remote connection/attempt, missing group, offline/closing source, unsettled catalog, pending close and duplicate unresolved host request; assert rejection precedes any mutation or connection attempt.
- [ ] 1.2 Extend the existing source creation reservation and Client serialization to admit fresh remote Screen/Pane IDs once through window-wide limits, omit default name and all local launch fields, and roll back failed enqueue atomically. Extend catalog/connection/embedded in-process tests for exact wire payload, Screen/pane/catalog bounds, local behavior preservation, no local profile/CWD side effects, and real server naming over unopened Screens and concurrent creations. Correct `docs/protocol.ts::CreateScreenOp` to existing `newPaneId`, optional name/profile and absence of obsolete ownership; verify against Rust operation types.
- [ ] 1.3 Update `architecture/client-core.c4` connection/client_workspace/native_workspace descriptions and `docs/architecture.md` creation admission/reservation ownership with this seam. Verify architecture tests and dependency checks pass, with no new service, transport protocol or UI dependency in the Core.

## 2. Canonical observation, uncertainty and runtime lifecycle

- [ ] 2.1 Extend remote pending-reference handling and the existing coherent collector to settle only a complete post-submission candidate, retaining mutation watermarks and newer dirty work while older observations drain. Extend `workspace/catalog/tests.rs` and `catalog/refresh/tests.rs` unit/in-process tests for old opening/trailing catalogs, a standalone snapshot, a missing snapshot, invalid/over-capacity candidate, bounded recollection/deadline, confirmed presence/absence, and unrelated catalog entries; assert no phantom routes, automatic opened entries or mutation replay.
- [ ] 2.2 Preserve only submitted observation intent and unresolved capacity across remote connection reset; clear old send, route, completion and follow authority. Extend existing window/remote in-process fixtures for loss before and after the server applies Create, replacement observations with presence/absence, delayed old events, retained per-host exclusion, exact reservation conversion/release and late canonical reappearance under exhausted capacity. Assert current absence does not claim historical nonexecution and failed reads never free unresolved admission.
- [ ] 2.3 Route the new command through the existing Remote/SourceRuntime path, publish bounded source/request-qualified pending/outcome state, and carry valid creation follow through `merge_scoped` and global selection. Reuse request-qualified pure model cancellation when Screen/destination navigation revokes follow. Extend existing native navigation/source/remote in-process tests for another selected source, two independent hosts with colliding raw IDs, canonical follow, navigation/newer creation/destination cancellation, and continued local/other-host output while creation is unobserved.
- [ ] 2.4 Complete detach, operation-rejection and shutdown handling using existing demand and cleanup owners. Extend existing remote/close fixtures for detach before confirmation, late snapshot after removal, a rejected create versus uncorrelated mux failure, normal window close while creation is uncertain, last old reference removed while the new reference retains demand, and a later explicit creation with fresh IDs. Verify no compensating remote close, no Create replay, host sessions survive and local capacity is released at the proper ownership boundary.
- [ ] 2.5 Update client-core architecture descriptions and the native Remote Screens section of `docs/architecture.md` for observed state versus unknown historical outcome, reset accounting and follow cancellation. Reconcile only predecessor `native-remote-screens` creation exclusions/unsupported-action wording to reference this narrow successor, without checking off or removing its unfinished tasks and without altering unrelated newer feature deltas. Verify architecture checks and strict validation of both affected changes.

## 3. Native host-group action and feedback

- [ ] 3.1 Add the explicit New Screen Kit action to the already opened Drawer host group, preserving caption navigation and the Ports menu. Use the shared capture/availability and submission path, immediate bounded enqueue feedback, canonical pending/name/outcome readers and ordinary terminal focus. Extend existing Drawer/Hosts production-view UI integration tests with actual pointer and keyboard activation, selected-source mismatch, alias collisions, duplicate activation, stale owner/attempt/group and offline/reconciling/capacity disablement; prove render/inspection/dismissal sends no command.
- [ ] 3.2 Wire destination changes and detach to the shared follow/lifecycle contract, keeping later selection/focus authoritative. Extend actual GPUI production-view input tests for completion with and without intervening navigation, Hosts/Settings departure, reconnect, unresolved reference removal and late completion. Check pending/unknown/missing feedback, a fresh explicit later request, keyboard focus, compact Drawer bounds, zoom and autohide; update the GPUI architecture description and native subset documentation and run architecture checks.

## 4. Integration evidence

- [ ] 4.1 Run focused client-core catalog/refresh/window/native runtime tests and GPUI Drawer/Hosts tests, strict touched-crate all-target Clippy, formatting, architecture tests and `openspec validate native-remote-screen-create --strict`; record exact outcomes and confirm ordinary local creation and assessed remote termination retain their regression coverage. Use existing fixtures and install architecture dependencies with `pnpm install --dir architecture` if needed.
- [ ] 4.2 Extend the existing owned remote-screen actual-window fixture for one frozen Linux candidate: create on one ready opened host while another source is selected, verify the new canonical host Screen/session and host-assigned name, navigate away during delayed confirmation, detach/reopen the new Screen with the same surviving session, and close normally. Record input/focus/viewport evidence and isolated cleanup; use existing bridge-loss fixtures for no-replay where supported. No new server-process smoke framework or external host is required; in-process tests carry deterministic loss/ordering proof.
- [ ] 4.3 At the later merge checkpoint, run once: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, and `openspec validate --specs`. Full workspace CI remains deferred until that checkpoint; do not mark this task complete on focused checks. No Tauri/frontend checks are added unless those sources become touched.

## Scenario coverage index

| Delta scenario | Task evidence |
| --- | --- |
| Independent numbering; Existing unopened screen; Existing names survive | 1.2 (existing naming regression plus real server fixture) |
| Native default naming is decided on the host | 1.2 |
| Create within the captured host group | 1.1, 1.2, 2.3, 3.1, 4.2 |
| Creation is unavailable or its capture is obsolete | 1.1, 3.1 |
| Duplicate creation or exhausted admission | 1.1, 1.2, 3.1 |
| Ordered confirmation and stale observation | 2.1 |
| Rejection or bounded observation failure | 2.1, 2.4 |
| Reconnect after an unobserved creation | 2.2 |
| Capacity through uncertain recovery | 2.2, 2.4 |
| Creation completes after navigation | 2.3, 3.2, 4.2 |
| Detach while creation is unresolved | 2.4, 3.2, 4.2 |
| A later explicit creation is a new request | 2.4, 3.2 |
