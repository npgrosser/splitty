# Tasks

## 1. Shared host administration and browse ownership

- [ ] 1.1 Add validated host-management request/result types under the existing hub module with exact local owner, request and target identity. Freeze the public read/command seam before GPUI edits. Extend existing hub **unit/in-process tests** for invalid target/alias, clearing versus preserving aliases, stale owner, duplicate ID/target and bounded admission before effects.
- [ ] 1.2 Extend the owned hub API scheduler with the bounded FIFO, distinct mutation outcomes and authoritative read invalidation. Extend `hub/tests.rs` scripted-peer **in-process tests** for successful/rejected/uncertain replies, same-service invalidation before/after acknowledgement, stale read suppression, fair independent progress, close-time discard/join and no replay on reopening.
- [ ] 1.3 Add explicit target preparation and the private shared remote admission continuation, retaining directory-gated known-row Connect. Add worker-owned withdrawal of one identified connection continuation and of all browse interests when navigating away. Extend `native_workspace/hub_tests.rs` and remote **in-process tests** for new/hidden targets, omitted alias, preparation failure/uncertainty, duplicate submission, leave/cancel/shutdown before late success, cancellation racing admission without releasing unrelated browsing, and preserved opened-reference demand.
- [ ] 1.4 Preserve visible current connection work independently of the saved directory. Extend real shared-model **unit/in-process tests** for failed unnamed targets, confirmed alias refresh, Forget with open references, no remote session mutation and no implicit saved-directory resurrection. Reuse existing server host metadata tests for unchanged forward/tombstone semantics rather than duplicating backend implementation tests.

## 2. Native forms and actions

- [ ] 2.1 Add a retained Connect dialog with target/known-target suggestions, optional display name, field validation and identified pending/error presentation. Extend production-view **GPUI context/UI integration tests** for pointer/keyboard submission, duplicate suppression, Escape focus return and cancellation while preparation is pending.
- [ ] 2.2 Add per-host display-name edit/clear and Remove from list; render current unsaved/forgotten work distinctly and reconcile exact-owner outcomes without optimistic saved claims. Extend retained-view **GPUI context/UI integration tests** for correct target capture, cleared alias, rejected/uncertain/stale result handling, dynamic rows and terminal focus preservation.
- [ ] 2.3 Extend the existing owned remote-window harness with actual UI target entry, alias edit/clear, Remove while a remote terminal remains usable, and explicit re-add of the hidden target. Record optimized Linux **real-window integration** evidence, including invalid input causing no SSH, normal close and exact owned fixture cleanup. This proves real input/focus/rendering that in-process service tests cannot establish; no external host is required.

## 3. Documentation and integration

- [ ] 3.1 Update the actual hub/native runtime and GPUI relationships in `architecture/client-core.c4` and `architecture/gpui.c4`, the ownership sections in `docs/architecture.md`, and supported native flows in `docs/gpui-spike.md`. Verify `docs/protocol.ts` still matches touched existing host metadata envelopes. Reconcile the active remote change's earlier exclusion without claiming its broader matrix complete.
- [ ] 3.2 Run targeted client-core hub/native-runtime and GPUI tests, touched existing server metadata tests where needed, strict changed-crate Clippy, `cargo fmt --all --check`, `cargo test -p splitty-architecture`, strict change validation and `openspec validate --specs`; require the new real-window flow to pass before completing this increment.

Full workspace test/Clippy and platform/release CI remain deferred until the native MVP/main integration, as already established for this branch. Before that merge, run the full checks from `CLAUDE.md` once; they are not a gate for this feature increment.
