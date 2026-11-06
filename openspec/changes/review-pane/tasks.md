# Tasks

## 1. The record and the operations (`splitty-core`, `mux`)

- [x] 1.1 `ViewerRecord` gets `annotations` (`ViewerAnnotation` with `id`, `kind`, `body`, `replacement`, `target` with `view`, `quote`, `prefix`, `suffix`, `startLine`, `endLine`), omitted while empty; serde names as in `docs/protocol.ts`. Unit tests for the wire shape in the mux module
- [x] 1.2 `ScreenOp::PutAnnotations` and `ScreenOp::RemoveAnnotations` with their wire names: validation (viewer pane, 1 to 100 entries, ID form and uniqueness within the operation, kinds and their required fields, byte limits per field and per pane, line bounds, the count after the operation) and the reducer (replace in place, append, ignore unknown IDs on removal, revision bump, no effects). Unit tests per scenario of "Annotations On A Viewer Pane" and "Saving Annotations Through The Window" ("A question on a passage", "A general comment", "One annotation too many", "A terminal pane", "Two windows add at the same time" as two operations on one state)
- [x] 1.3 Lifetime: moves, floating, docking, stacking, `move_pane_to_screen` and the reuse in `open_viewer` keep the annotations; `close_pane` and `close_screen` end them. Unit tests ("Opened again from the CLI", "Pane closed")
- [x] 1.4 `state.json` keeps the annotations, and a restore brings them back. Unit test in `daemon` ("Daemon restart", "Annotations persisted")

## 2. The server (`splitty-server`)

- [x] 2.1 `mux.welcome` announces `viewer-annotations-v1`; after `put_annotations` and `remove_annotations` the other clients get `mux.nudge` (`mux_op`) and the sender nothing; a rejected one gets `mux.op_failed` with the pane to the sender only; snapshots carry the annotations. In-process test in `client_session::mux_handler` beside the viewer nudge test ("Annotation added by a client", "A second window sees the annotation", "Rejected annotation")
- [x] 2.2 Control API: `GET /api/mux/state` and `GET /api/mux/screen/{id}` leave out `annotations`; `POST /api/mux/op` refuses both operations with 400. In-process router tests beside the scratchpad tests ("Annotations omitted", "Annotations refused", "An agent reads the workspace", "An agent tries to annotate", "Annotations only through the window")

## 3. The window (`frontend/`)

- [x] 3.1 Protocol: `annotations` in the viewer record of `lib/mux/incoming.ts` (bounded, validated, malformed entries dropped), the two operations, `viewer-annotations-v1` on `MuxClient`. Vitest in `lib/mux` ("Host without annotations")
- [x] 3.2 The store per connection (`lib/viewer/annotations.ts`): annotations from snapshots, the window's own operations, the client-side limits, the settle time against older snapshots, IDs. Vitest with fake timers ("An older snapshot arrives", "Two windows add at the same time")
- [x] 3.3 Anchoring (`lib/viewer/anchoring.ts`): quote search in the source and in rendered text, context scoring, the nearest line, whitespace fallback, not found and found again, line mapping for both views. Vitest for every scenario of "Anchoring By Quote"
- [x] 3.4 The review (`lib/viewer/review.ts`): heading, path, numbering, lines, the note for passages not found, HTML passages without lines, quotes, fenced replacements with a safe fence, general comments last. Vitest ("A plan with a question and a suggested change", "A terminal selection")
- [x] 3.5 Making annotations: the selection toolbar and the context menu entries in both views, selection reading in the shadow root (`getComposedRanges`, `ShadowRoot.getSelection`), the form (kinds, replacement prefilled, Ctrl/Cmd+Enter, Escape, the quote limit), edit and delete with undo. Vitest in a DOM environment for the selection-to-target mapping and the form's validation
- [x] 3.6 Showing annotations: CodeMirror decorations and gutter markers, the CSS Custom Highlight API in the rendered view with block markers as fallback, the annotation list and its order, the tab marker with the count. Vitest for the list order and the decoration ranges ("From the rendered view to the source")
- [x] 3.7 Hand-over: the target (origin, chooser among the screen's terminals), paste with focus, the bracketed paste check of both emulators, the file through `upload` with path insertion, the scratchpad append within its limit, copy, download, clear; ending the round with `remove_annotations` and the ten-second undo. Vitest with a fake terminal target ("Send to the agent", "Undo after sending", "The origin is gone", "A program without bracketed paste", "Into the scratchpad", "Copied", "Added while sending")
- [x] 3.8 Confirmations: the close guard and the embedded quit confirmation count annotations. Vitest in `close_guard.test.ts` ("Close a viewer with annotations", "Quit an embedded window")
- [x] 3.9 Check in a real browser against a local `serve` (Playwright, one-off script): annotate a Markdown plan in both views, a second window shows the annotations, a rewrite of the file moves a highlight and marks a removed passage, the review is pasted into a terminal without Enter, undo brings the annotations back, closing the pane asks first

## 4. Documentation

- [x] 4.1 `docs/protocol.ts` (`ViewerAnnotation`, the two operations, `viewer-annotations-v1`), `docs/architecture.md` (annotations in the section Viewer Panes), the `web` container in `architecture/landscape.c4`, `docs/files-and-editors.md` (reviewing a document), the agent guide (what an agent receives), README

## 5. Verification

- [x] 5.1 Targeted: `cargo test -p splitty-core mux::`, `cargo test -p splitty-core daemon::`, the new tests of `splitty-server`, `pnpm test` for the touched files in `frontend/`
- [x] 5.2 Once before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`; in `frontend/`, `pnpm check`, `pnpm test` and `pnpm build`; `openspec validate --specs`. `crates/splitty-tauri` stays untouched
