# Proposal

## Why

A process that waits for a review (`splitty review`) gets one of two answers today: an approval without anything else, or a review. A review does not say what the user decided. The agent has to guess from the comments whether to go ahead or to revise, a script cannot tell the two apart by the exit status, and a user who wants to turn a plan down as a whole has no way to say so apart from writing a comment. Comments on the document as a whole come last in the review, after every comment on a passage, although they are usually the user's answer to the document.

A waiting process is also easy to miss: the banner shows only in the viewer pane, so a pane in a background tab gives no sign that an agent is blocked on it. And when a file changes on disk while its review waits, the review refers to a version the agent did not show, without saying so.

## What Changes

- **A verdict**: a window answers a wait with **Approve** or **Request Changes**. Approving goes ahead; comments the user has not handed over yet go along as notes. Requesting changes always answers with a review, which may hold no comment at all (a plain rejection). Both replace the former pair of Approve and Send Review.
- **An overall comment**: the banner holds an optional text field for the user's answer to the document as a whole. Its text becomes a general comment, with a number and a thread like every other, before the answer goes out.
- **The review states the verdict and leads with the general comments**: the review of an answer starts with a verdict line, and every review lists the general comments before the comments on passages. The pane's list uses the same order.
- **A document that changed during the review**: the banner says so, and the review of the answer notes that its lines and quotes refer to the version shown when it was answered.
- **Waiting is visible on the tab**: while a process waits for the review of a viewer pane, the pane's tab shows a marker that leads to the pane.
- **Protocol and API**: `viewer.review_answer` and `POST /api/viewer/review/wait` carry the outcomes `approved`, with an optional review, and `changes_requested`, with a review. **BREAKING** for the unreleased outcome `feedback`, which is removed.
- **`splitty review`**: prints the review for either verdict, nothing for a plain approval, and exits 0 on an approval and 3 when the user requests changes, with or without `--json`.

Out of scope:
- A verdict for reviews sent to a terminal. The user writes at the prompt along with the pasted review.
- A third verdict that discards the document; a request for changes with a general comment says what to do instead.
- Pinning a file review to the version the waiting process showed on the server; the window notes a change it sees.
- Notifications outside the window (sound, system notifications); see the draft `terminal-bell`.
- The integration with an agent's plan approval (a hook that runs `splitty review`), which follows as its own change.

Roadmap: follow-up of 111 (`review-threads`), see `roadmap/README.md`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `viewer-annotations`: Waiting For A Review (verdict, overall comment, changed document), The Review (verdict line, general comments first, the note on a changed document), Annotations In The Pane (list order, marker of a waiting review on the tab).
- `mux-protocol`: Review Answers (outcomes `approved` and `changes_requested`).
- `control-api`: Review Requests (the response for both verdicts).
- `cli`: Review Command (output and exit status per verdict).

## Impact

- Server: the outcome of an answer and of the held request (`client_session/reviews.rs`, `service/review.rs`), `splitty review` and the agent guide.
- Window: the banner, the answer, the review builder, the list order, the tab marker.
- Docs: `docs/protocol.ts`, `docs/files-and-editors.md`.
- Tests at all three levels, and the browser check of the review flow.
