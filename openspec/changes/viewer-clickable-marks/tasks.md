# Tasks

## 1. Marks under the pointer

- [x] 1.1 `lib/viewer/mark_hits.ts`: the order clicks go through the marks under the pointer (a passage of the shown view before lines, the shortest first) and the cycle on further clicks on the same marks; unit tests in `mark_hits.test.ts` (none, shortest first, cycling, starting over elsewhere)
- [x] 1.2 `SourceView`: the annotation's ID on a passage's decoration, a hover style, and `marksAt` for passages and the bars beside their lines; unit test in `source_view.test.ts`
- [x] 1.3 `RenderedDocument`: a hover highlight and block style, and `marksAt` by the passages' client rectangles, the caret where nothing is laid out, and the marked blocks; none on a link; unit tests in `render.test.ts`

## 2. The layer

- [x] 2.1 A click on a mark (without a selection, a drag, a double click or a modifier) opens the list on the thread, chooses it without scrolling the document and gives its entry the keyboard; the pointer previews the mark a click chooses and names the annotations under it; in-process tests in `annotation_layer.test.ts` (Scenario A mark chosen, Overlapping passages, clicks left to the document, the bars, a marked block of the rendered view, the pointer)
- [x] 2.2 The eye in the pane's bar replaces the list header's Hide Marks button; while the marks are hidden, the passage of the annotation chosen since is marked; extend the Marks hidden test in `annotation_layer.test.ts` and add one for Scenario An annotation chosen while the marks are hidden
- [x] 2.3 `docs/files-and-editors.md` and `docs/architecture.md`

## 3. Checks

- [x] 3.1 Targeted: `pnpm test src/lib/viewer/mark_hits.test.ts src/lib/viewer/source_view.test.ts src/lib/viewer/render.test.ts src/panes/viewer/annotation_layer.test.ts` in `frontend/`, and the browser check of the review flow with clicks on a rendered and a source mark and the eye in the pane's bar
- [x] 3.2 Before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`; `pnpm check`, `pnpm test` and `pnpm build` in `frontend/`; `openspec validate --specs`
