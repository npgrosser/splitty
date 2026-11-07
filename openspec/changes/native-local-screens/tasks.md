## 1. Ordered catalog and typed connection API

- [ ] 1.1 Add the capability, serial workspace catalog request/response, coherent store projection and typed lifecycle client methods; update `docs/protocol.ts`. Extend unit and in-process protocol tests for lifecycle ordering, empty catalog, duplicate/partial order with filtered active hint, capability compatibility and uninterrupted session delivery.

## 2. Connection-wide workspace model

- [ ] 2.1 Separate per-Screen state from the connection-wide catalog, local shown choice, routing and global reservations. Extend unit tests for independent tab/focus state, catalog confirmation, stale generation/snapshot/spawn rejection, pending creation under an older catalog response, request coalescing/watermarks and admission recovery across Screens.
- [ ] 2.2 Extend real-service in-process client tests for create/rename/switch/close/final-close/new-screen, stable PTY/session identity and hidden output/replies, hide/resize withdrawal, removal cleanup and surviving attachments. Existing single-Screen split/tab/drag tests remain passing.

## 3. Native workspace integration

- [ ] 3.1 Integrate one worker and shared bounded queues with the new model, Screen controls and local rename/close interactions. Extend unit tests for generation-bound callbacks, activation visibility changes, unsupported capability, final empty state and preservation of edit focus during unrelated updates.
- [ ] 3.2 Extend the actual native-window smoke to exercise Screen creation, rename, switching, hidden output, closing and final reopen, preserving terminal identity and normal owned-service cleanup. This smoke is needed because in-process tests cannot validate native hit targets, text-editor focus, prompts or rendered entity continuity.
- [ ] 3.3 Update native workspace concepts in `docs/architecture.md` and component descriptions/actual relationships in `architecture/*.c4`; verify the focused architecture test passes.

## 4. Focused MVP verification

- [ ] 4.1 Run formatting, targeted unit/in-process tests and targeted Clippy for touched components, the native-window smoke and OpenSpec validation. Record observed behavior and limitations. Broad workspace/CI and additional platform gates are deferred to native cutover; this feature increment does not wait for them.
