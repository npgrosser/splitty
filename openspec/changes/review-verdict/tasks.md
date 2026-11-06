## 1. Server

- [x] 1.1 Outcomes `approved` with an optional review and `changes_requested` with a review in the registry of waits and its parser; `feedback` breaks the rules (`client_session/reviews.rs`)
- [x] 1.2 The held request's response for both verdicts (`service/review.rs`)
- [x] 1.3 Unit tests (extend `client_session/reviews.rs`): an approval with and without a review settles, a request for changes without a review or over 1 MiB settles nothing, `feedback` settles nothing (mux-protocol, Review Answers: An approval with notes, A request for changes without a review)
- [x] 1.4 In-process tests (extend `http/tests/reviews.rs` and the `mux_handler` answer test): the responses for an approval with notes and for a request for changes (control-api, Review Requests: Approved, Changes requested; mux-protocol, Review Answers: The first answer)

## 2. CLI

- [x] 2.1 `splitty review` prints the review of either verdict, nothing for a plain approval, and exits 3 on a request for changes, with or without `--json`; standard output is flushed before the exit
- [x] 2.2 The agent guide: the verdicts, the verdict line, exit status 3, the JSON outcomes, and what to do on each
- [x] 2.3 Unit tests (extend `cli/review.rs`): output and exit status per outcome, plain and `--json` (cli, Review Command: A plan reviewed, Approved, Approved with notes, Outcome as JSON)
- [x] 2.4 Smoke test (extend the review case of `tests/http_surface.rs`): a real `splitty review` answered with `changes_requested` exits 3 and prints the review, which only the binary's exit path shows

## 3. Window

- [x] 3.1 Protocol types and the answer: outcomes `approved` and `changes_requested`, the review with an approval when given (`lib/protocol/types.ts`, `lib/viewer/handover.ts`)
- [x] 3.2 The review builder: the verdict line, the note on a changed document, general comments first, no reply line without annotations; `compareListPlaces` puts general comments first (`lib/viewer/review.ts`)
- [x] 3.3 The banner: the overall comment field, Approve and Request Changes, the note on a changed document (`panes/viewer/review_banner.ts`)
- [x] 3.4 The answer per verdict in the annotation layer: the overall comment saved as a general comment and numbered before the review, the field's text kept per pane while no answer took it, an approval with the pending entries as notes, a request for changes with the review's selection, the document version recorded per wait (`panes/viewer/annotation_layer.ts`)
- [x] 3.5 The wait of each viewer pane in the annotation store, and the tab's marker for a waiting review (`lib/viewer/annotations.ts`, `layout/tab_renderer.ts`)
- [x] 3.6 Unit tests (extend `lib/viewer/review.test.ts`): the verdict and note lines, general comments first, a plain rejection without the reply line (viewer-annotations, The Review: General comments lead; Waiting For A Review: A plain rejection)
- [x] 3.7 Unit tests (extend `lib/viewer/handover.test.ts` and `lib/viewer/annotations.test.ts`): the answer's fields per outcome; the wait per pane from snapshots
- [x] 3.8 In-process tests (extend `panes/viewer/annotation_layer.test.ts`): Request Changes with comments and without any, an overall comment saved and numbered before the answer and kept when the wait ends, Approve with and without pending entries, the note when the document's version changes during the wait, general comments first in the list (viewer-annotations, Waiting For A Review: Feedback on a plan, A plain rejection, An overall comment, Approved, Approved with notes, The file changed during the review; Annotations In The Pane: General comments first)
- [x] 3.9 In-process test of the tab marker (extend the tab renderer's test, or add one next to it): shown while the record names a wait, gone after it, a click shows the pane (viewer-annotations, Annotations In The Pane: A waiting review in a background tab)

## 4. Docs

- [x] 4.1 `docs/protocol.ts`: `viewer.review_answer` and the response of `POST /api/viewer/review/wait`
- [x] 4.2 `docs/files-and-editors.md`: the verdicts, the overall comment, the exit status of `splitty review`
- [x] 4.3 `openspec/open-decisions.md`: bullets for D1 to D6

## 5. Checks

- [x] 5.1 Targeted tests: `cargo test -p splitty-server reviews`, `cargo test -p splitty-server review`, `cargo test -p splitty-server --test http_surface review`, `pnpm test src/lib/viewer src/panes/viewer src/layout` in `frontend/`
- [x] 5.2 The browser check of the review flow (`threads_check.mjs`) with Request Changes, Approve with notes and the overall comment
- [x] 5.3 Full check: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`, `pnpm check`, `pnpm test` and `pnpm build` in `frontend/`, `openspec validate --specs`
