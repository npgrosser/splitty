# Tasks

## 1. Local settings ownership

- [ ] 1.1 Introduce the concrete shared auxiliary reader and separately charged profile snapshot/read with bounded presentation; extend unit/in-process API tests for valid/malformed/oversized data, generation coalescing, fair scheduling, OS-only appearance refresh, foreign-owner rejection, independent failure/recovery, stream separation and cancellation cleanup.
- [ ] 1.2 Integrate independent settings updates into the native worker, preserving retained setup ownership and cleanup; extend in-process runtime tests for startup, same-service refresh and shutdown during a pending read.

## 2. Explicit launch and presentation

- [ ] 2.1 Add captured explicit-profile new-tab intent and admitted bounded fallback reporting; extend in-process workspace/worker tests for stale client CWD versus current service inheritance, explicit profile precedence, unavailable-profile fallback and stale/foreign source rejection without duplicate launch.
- [ ] 2.2 Add the native new-tab profile picker with enabled/available/default/experimental presentation, keyboard/mouse dismissal and feature-local status; verify selection and focus behavior with a real-window smoke check because in-process service tests cannot observe native event propagation or PTY typing after dismissal.

## 3. Integration and documentation

- [ ] 3.1 Update architecture components/dependencies, native-runtime concepts and evaluation instructions; verify the module-level architecture tests and existing protocol names against the unchanged wire schema.
- [ ] 3.2 Run targeted client/native tests and Clippy, formatting, affected strict spec validation and normal main-spec validation; build and run the frozen native preview with profile interaction evidence and owned cleanup. Broad workspace/CI checks remain deferred during the native evaluation.
