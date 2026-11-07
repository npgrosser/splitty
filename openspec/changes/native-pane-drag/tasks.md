# Tasks

## 1. Captured move contract

- [x] 1.1 Add captured pane move validation, size preservation and authoritative placement selection in client-core; unit tests cover stale connection/structure/route and allowed metadata revisions, no-op/self edges, proportions, earlier snapshots and rejected moves.
- [x] 1.2 Extend the existing real-service workspace in-process test to move real PTY panes into tabs and edges, verifying session identity, source collapse, proportions and selected focus.

## 2. Native gesture integration

- [x] 2.1 Wire one bounded worker move command and rejection cleanup; add typed tab drag previews, current-slot center/edge targets and Escape cancellation, with pure zone unit tests and stable entity reconciliation.
- [x] 2.2 Exercise actual native mouse drags in an isolated display, verifying moved shell identity, retained terminal entities, cancellation and normal shutdown; this real-window smoke is needed because in-process reducer tests cannot exercise pointer dispatch or painting.
- [x] 2.3 Update architecture component descriptions, client connection docs and native development usage; verify no new component/dependency is introduced.

## 3. Integration checks

- [x] 3.1 Run targeted client-core and GPUI tests, formatter and clippy; validate the change and specs.
- [x] 3.2 Record focused native MVP validation: client-core/GPUI tests and clippy, actual mouse-drag smoke, architecture consistency and OpenSpec validation. Broader workspace, CI and cross-platform cutover gates remain separate follow-up work.
