# Tasks

## 1. Shared transfer and reconciliation

- [ ] 1.1 Add captured-source transfer admission and identified outcomes in client workspace/runtime; extend unit tests for non-selected tabs, stale owner/route, managed jobs, invalid targets, conflicts and one-submit queue rejection.
- [ ] 1.2 Add bounded coherent workspace collection using opening and trailing catalog fences; extend unit tests for source/target/catalog delivery orders, external moves, new targets, emptied sources, missing responses, aggregate limits and inconsistent-candidate recovery with no mutation replay.
- [ ] 1.3 Preserve stable routes/readiness and local navigation/maximization through commit; extend unit tests for unchanged session/attachment, confirmed reveal, later-navigation cancellation and external focus preservation.
- [ ] 1.4 Integrate pane/Screen close exclusion and assessed whole-window shutdown during reconciliation; extend existing unit/in-process runtime tests for conflict rejection and confirmed joined shutdown.
- [ ] 1.5 Update client-core architecture descriptions and Client connections documentation to match ownership, bounded observation and settlement; verify the architecture check.

## 2. Native transfer menu

- [ ] 2.1 Add a captured-source tab transfer menu with existing/new Screen destinations and visible/right-click entry points; verify dismissal, target capture and identified result handling with focused unit tests and real-window use.
- [ ] 2.2 Preserve terminal-body pointer routing and retain terminal entities across confirmed transfers; update GPUI architecture description and verify native unit tests.
- [ ] 2.3 Extend private real-window tooling with transfer acceptance for non-selected tabs, existing/new targets, final-source removal, same PTY identity/history/search state and normal cleanup. This smoke-level check is needed because in-process model tests cannot show menu routing, entity retention or actual terminal rendering.

## 3. Integration and acceptance

- [ ] 3.1 Run affected client-core/native tests, targeted all-target Clippy, formatting, architecture and OpenSpec validation; record platform limits and verify no protocol or web behavior changes were introduced.
- [ ] 3.2 Build and freeze a release candidate, run the real-window transfer acceptance and existing GPU workspace smoke against that exact binary, review screenshots, then update the stable preview and archive/sync the change.

Full workspace merge checks and CI remain deferred while this experimental branch develops. Before a later merge, run the full checks in CLAUDE.md once; this increment requires the targeted checks above.
