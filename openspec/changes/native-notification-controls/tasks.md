# Tasks

## 1. Shared editor contracts

- [x] 1.1 Extract existing common field/edit/value/metadata/limit types into the UI-free settings module, retain terminal presentation preferences under appearance and update real imports without changing the existing twelve fields. Verify existing local-service settings unit/in-process tests and GPUI compilation retain behavior.
- [x] 1.2 Add the settings component and actual dependencies to `architecture/client-core.c4` and `architecture/gpui.c4`, and document the contract boundary in `docs/architecture.md`. Verify all architecture checks pass with no forwarding-only or artificial dependency edge.

## 2. Independent notification edits

- [x] 2.1 Add four notification fields, explicit field domains and bounded boolean patches; keep appearance and notification value lookup separate. Extend existing settings unit tests to verify exact paths, invalid types, domain mapping, single-field patches and preservation of toast/desktop preferences.
- [x] 2.2 Extend the existing notification job with typed effective values and four optional server-default reset values, independent from theme resolution. Extend local-service in-process fixtures for valid switches plus failed theme, missing/malformed metadata, exact defaults, stale/foreign results and charged allocation release; verify no settings write occurs during reads.
- [x] 2.3 Route admission, write acknowledgement decoding, fresh readback generations, deferred updates and uncertainty release by field domain on the existing writer. Extend existing local-service unit/in-process tests for mixed-domain FIFO/field exclusion, wrong owner, appearance-only OS refresh, same-service broadcasts, stale deferred updates, malformed replies, uncertainty/read failure/recovery, single submission and owned shutdown. Preserve valid-home reconciliation even when later appearance theme resolution fails.
- [x] 2.4 Update local-service and settings component descriptions plus the domain/readback account in `docs/architecture.md`; verify architecture checks reflect actual references and no new server/protocol dependency or mutation scheduler exists.

## 3. Native controls and authoritative policy

- [x] 3.1 Retain separate accepted notification settings in the existing Settings view, including observations while closed, and dispatch effective-value/default/completion lookup by field domain. Extend production-view unit/in-process tests for late opening, failed appearance with valid switches, missing defaults, stale owner/read/write/reconciliation and independent domain generations.
- [x] 3.2 Add controlled master and three process-category switches with existing field-specific pending/error/uncertain feedback and exact-default Reset. Extend production-view UI integration tests for toggle and Reset through the actual Settings event path, rejection preserving confirmed values, uncertainty without retry, master changes preserving categories and unsupported toast/desktop settings, and keyboard/focus behavior.
- [x] 3.3 Apply switches to unread policy only from accepted authoritative notification readback, never optimistic control state. Extend the existing unread production-view fixture to verify disabling clears affected badges only after readback, failed writes leave them unchanged and foreign results cannot change policy.
- [x] 3.4 Document native notification editor scope and update existing GPUI component descriptions/relationships; verify the architecture check and focused view tests match the ownership boundary.

## 4. Integration evidence

- [ ] 4.1 Exercise the production controls in one owned real native window using the existing settings fixture approach; verify at least a switch and service-default Reset persist exactly the selected field, layout/overflow and focus remain usable, and owned cleanup completes. This reuses existing window automation because in-process assertions alone do not establish rendered component placement; no new server-process test framework is required.
- [x] 4.2 Run focused client settings/scheduler tests, GPUI settings/unread tests, touched-crate strict Clippy, formatting, architecture checks and strict change validation; verify results pass and record precise scope.
- [ ] 4.3 At the later main-merge checkpoint run once: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui` (install architecture dependencies with `pnpm install --dir architecture` if needed), and `openspec validate --specs`. This full gate remains deferred until integration review.

Integrated source verification: all 165 GPUI tests and 17 architecture checks pass. Three new production-view tests use actual Switch/Reset input and the existing Settings event path while a remote Screen is selected, check independent domain completion and failed/missing metadata/appearance, exercise native key down/up activation and pending suppression, reject stale owner controls and verify no badge purge before accepted readback. Existing appearance field assertions use optional domain lookup; strict GPUI/client-core Clippy, formatting and strict change validation pass. Owned actual-window acceptance and the full merge gate remain open.
