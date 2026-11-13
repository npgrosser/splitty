# Tasks

## 1. Server: anchoring and review text

- [x] 1.1 Pure module in the core's mux: the source text of a document's bytes (SBR-4), the anchoring of SBR-3 with states `anchored` / `changed` / `lost` / `general`, the list order of SBR-6, and the review of SBR-2 with the entries it names; `viewer-reviews-v1` constant.
- [x] 1.2 Unit tests in that module, ported from both windows' suites (client-core `annotations` tests, `frontend/src/lib/viewer/anchoring.test.ts`, `frontend/src/lib/viewer/review.test.ts`) as the union of their cases; where the windows disagreed, the test states the SBR decision. Covers the scenarios Lines inserted above the passage, The passage reworded, The passage removed, The same words twice, A reflowed paragraph, A script without spaces, A plan with a question and a suggested change, General comments lead, A thread with an answer, A terminal selection, A reply that poses as the user, A quote with an escape sequence, A reworded passage, Sent again, Ranges in the source.
- [x] 1.3 `viewer.anchor` and `viewer.review` in the session connection's viewer reads: parse and validate, take annotations and source at receipt, read the whole document off the loop, anchor, build, answer `viewer.anchored` / `viewer.reviewed` / `viewer.failed`; route them in the connection handler; announce `viewer-reviews-v1` in the welcome; add both to the session operation catalog and `docs/native-operations.json`.
- [x] 1.4 In-process tests (`client_session::viewer` tests): Anchors of a shown file, A review on request, Passages of a rendered view, A pane that is gone, A malformed request, A passage of a removed annotation, Added just before the review (an op and a review on one connection through the handler); the welcome capability list test; the catalog comparison test.
- [x] 1.5 `docs/protocol.ts`: the two messages, their answers and the capability.

## 2. Native window

- [x] 2.1 Client-core: decode viewer documents with the core's source text; commands for anchors and a review, the worker sends `viewer.anchor` / `viewer.review` over the pane's connection and reports `viewer.anchored` / `viewer.reviewed` / `viewer.failed` as updates; the capability read from the welcome. Delete the anchoring, list order and review text from client-core `annotations` (keep target description, `is_pending`, the hand-over policy, IDs).
- [x] 2.2 Window: anchors from the answer (marks mapped into the rendered view, list in the answer's order); copy, send and answer ask for the review and continue with it; on a host without the capability, no annotations and the update hint (scenario Older host).
- [x] 2.3 Tests: client-core unit tests for parsing the answers and for the worker's routing (in-process with a fake connection where one exists); gpui view tests where the notes have them (the notes have none; the worker has no fake-connection routing test, so the answers' parsing is tested on `Viewers` directly).

## 3. Web window

- [x] 3.1 A per-connection client of the two messages beside `ReviewAnswers`; the capability in the mux client; annotations offered only with both capabilities and the update hint otherwise.
- [x] 3.2 Annotation layer: source-view anchors from the server (byte ranges converted to UTF-16 offsets of the source text), rendered-view anchoring kept, the review from the server with the rendered passages; delete the review text builder of `lib/viewer/review.ts`.
- [x] 3.3 vitest: the conversion of ranges, the client of the messages, the layer with server anchors and server reviews (extend `annotation_layer.test.ts`), the older host.

## 4. Docs and checks

- [x] 4.1 `docs/architecture.md` viewer annotation sections (server, web, native), `docs/native-ui-handoff.md`, `openspec/open-decisions.md` bullet.
- [x] 4.2 Targeted tests: `cargo test -p splitty-core mux::review`, `cargo test -p splitty-server client_session::viewer`, `cargo test -p splitty-client-core annotations`, `cargo test -p splitty-gpui --features ui-test-support viewer`, `pnpm test` of the touched files.
- [x] 4.3 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo clippy -p splitty-gpui --all-targets --features ui-test-support -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `cargo test -p splitty-gpui --features ui-test-support`, in `frontend/` `pnpm check`, `pnpm test`, `pnpm build`, and `openspec validate --specs` (the architecture check needs `pnpm install --dir architecture` once).
