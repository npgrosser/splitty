# Tasks

## 1. Shared rules and web consent

- [x] 1.1 Extract the pure session-free template projection and command-retention helper into splitty-core, reuse projection in authoritative application, and extend unit tests for supplied-ID validation, tiled/floating parity, web pruning and command/restore-mode filtering.
- [x] 1.2 Bind the web guard to an immutable template snapshot, consume decisions once and refuse commands without a handler; update its caller's visible error and unit tests for every consent choice, absent handlers, source mutation and repeated decisions.
- [x] 1.3 Update architecture/core.c4 and the shared-template/consent explanation in docs/architecture.md; check existing template operation shapes against docs/protocol.ts and verify core architecture boundaries.

## 2. Native template library

- [x] 2.1 Add bounded immutable charged template drafts/catalogs and explicit API requests/results, rename the concrete auxiliary client to LocalServiceClient and extend its fair scheduler; verify unit/in-process bounds, cancellation, stale results, settings/zoom progress and joined shutdown.
- [x] 2.2 Implement list/load/extract/save/delete with captured drafts, command subsets and explicit overwrite admission; extend in-process tests for extraction root, persistence, unchanged reviewed commands, overwrite/delete cancellation, invalid extracted layouts and uncertain mutation outcomes.
- [x] 2.3 Document the library and local-service ownership/relationships in architecture/client-core.c4 and docs/architecture.md; verify architecture checks and the one-way templates-to-workspace data dependency.

## 3. Native application lifecycle

- [x] 3.1 Add sealed apply plans with allocated IDs, capability/capacity checks and captured Screen replacement impact; extend unit/in-process tests for changed/busy/unresolved targets, queue rejection, duplicate decisions and one apply operation without preliminary close/create.
- [x] 3.2 Add a separate pending projection, visibility/resize admission and ordered authoritative settlement; extend unit/in-process tests for immediate preview, later navigation, no speculative routes, partial launch/post-commit errors, visible profile-fallback notices, unverified outcomes and no replay.
- [x] 3.3 Include pending application in close assessment and joined shutdown, preserve unrelated sessions and publish coherent workspace before outcomes; extend in-process service tests for new-Screen and replacement applications, successful siblings, scoped conflicts and stale whole-window close plans.

## 4. Native presentation and actual-window acceptance

- [ ] 4.1 Add the Templates control/shortcut and one modal for list, extract/root, name/command selection, overwrite/delete and immutable apply review; extend GPUI unit tests for stale requests, explicit choices, modal input ownership and focus restoration, and document the controls in README.md/docs/gpui-spike.md.
- [ ] 4.2 Render pending tiled/floating layouts and new-Screen chips without canonical state mutation; connect precise outcomes and visibility withdrawal, extend GPUI unit tests for projected active tabs and navigation/close conflicts, and update architecture/gpui.c4.
- [ ] 4.3 Add private real-window smoke acceptance for extract/save/reload, layout-only versus actual command execution, replacement cancel/confirm, input isolation, pending presentation, unrelated process identity and complete cleanup. Unit/in-process tests cannot establish real pointer/focus/paint routing or command execution through the window; use isolated real shell sessions and captured visual evidence.

## 5. Integration and preview

- [x] 5.1 Run targeted core/client/GPUI tests and strict Clippy for affected crates, the web guard tests and frontend type checks, cargo fmt --all -- --check, architecture tests and openspec validate --specs; validate this change strictly. Full workspace/frontend merge CI remains deferred while this evaluation branch evolves.
- [ ] 5.2 Freeze one release candidate from the verified source, run private real-window template acceptance and hardware Wayland/X11 workspace smoke against that exact binary, inspect screenshots and promote the preview only after success; record source/binary hashes and platform limits in the runbook.
