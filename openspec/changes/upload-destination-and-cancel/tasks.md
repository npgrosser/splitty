# Tasks

## 1. Server and protocol
- [x] 1.1 Add capability, explicit cancellation and durable destination staging without replacement; extend unit/in-process upload tests for completion, invalid directories, conflicts, symlinks, cancellation and connection isolation; update docs/protocol.ts and architecture/core.c4. Verify targeted Rust upload and codec tests.

## 2. Client and drop workflow
- [x] 2.1 Add abortable client uploads and capability gating, shared destination dialog and pane progress, terminal/scratchpad wiring and safe path insertion; extend frontend unit tests for drop confirmation/dismissal, optional insertion, cancellation including batches, old hosts and unchanged clipboard behavior. Update README and architecture documentation/model. Verify targeted frontend tests and type checks.

## 3. Integration validation
- [x] 3.1 Run cargo fmt --all --check, cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings, cargo test --workspace --exclude splitty-gui; pnpm check, pnpm test and pnpm build in frontend; openspec validate --specs and validate this change. Record any environmental limitations.

## Verification results
- Rust fmt, workspace clippy and workspace tests (excluding splitty-gui): passed, including architecture checks.
- Frontend: 1,271 tests passed; check and production build passed. Eight existing Svelte warnings remain in unrelated files; none in the new dialog.
- Browser: exercised the actual Svelte component and drop handlers against a controlled upload target in Chromium: no transfer before confirmation, dismissal and Escape, temporary insertion, prefilled durable destination without insertion, cancellation and older-host gating. Dialog and progress screenshots inspected. Server storage and wire behavior were verified by Rust unit/in-process tests.
- OpenSpec strict change validation and all main specs: passed.
