# Tasks

## 1. Shared native API contract and transactions

- [x] 1.1 Move the pure API catalog/validation into splitty-core and preserve server reexports; retain its unit tests and verify the generated operation inventory and docs/protocol.ts remain equivalent. Update core/server architecture relationships and the shared-contract documentation.
- [x] 1.2 Extract the existing API-only transaction into splitty-transport and delegate both server and owned connections to it; extend unit/in-process tests for request correlation, cancellation, deadlines, charged replies and lost mutation outcomes. Update transport/remote/server architecture relationships with no reverse domain dependency.

## 2. Portable owned workspace

- [x] 2.1 Add concrete API-stream and owned-service wrappers, validated explicit WSL launch configuration and one cross-platform workspace/auxiliary worker; extend unit/in-process tests for arguments, incomplete configuration, two streams on one backend, API/terminal independence and startup cancellation. Update client-core architecture and Client connections documentation.
- [x] 2.2 Expose observed helper cleanup results without breaking existing callers and use them for native cleanup completion; extend unit/in-process tests for successful, unsuccessful and cancelled ownership cleanup, and verify no distribution-wide termination path is introduced.
- [x] 2.3 Wire GPUI startup arguments to the typed backend configuration while preserving Unix defaults; test argument combinations and validation at unit level, compile the native Windows path, and document explicit launcher usage and experimental limits in the native runbook/development guide and GPUI architecture model.

## 3. Build and real Windows verification

- [x] 3.1 Prepare pinned Windows Ghostty and an isolated MSVC source/build directory, build the optimized executable and matching Linux backend, and generate a launcher/package with recorded source and binary hashes; verify the Ghostty static-link check and actual launcher paths.
- [x] 3.2 Run a real Windows smoke/window check with the explicit WSL backend: actual input/output, split/tab/Screen operations, supported API/template access, graphics identity and joined cleanup while unrelated WSL work survives. Unit/in-process tests cannot establish Windows executable startup, native window rendering, cross-OS pipes or host process cleanup; retain evidence and report unverified interaction/performance limits.

## 4. Integration

- [x] 4.1 Run affected Linux and Windows checks, targeted strict Clippy, cargo fmt --all -- --check, architecture tests and strict change/spec validation; include Linux workspace regression after the portable adapter change. Full merge CI and production packaging remain deferred on the evaluation branch.
- [ ] 4.2 Integrate the Windows worktree with any template acceptance fixes, deliver the verified executable and launcher in Downloads, preserve the existing preview process and report exact startup instructions and remaining scope.
