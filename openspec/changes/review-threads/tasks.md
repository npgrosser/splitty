# Tasks

## 1. The record and the operations (`splitty-core`, `mux`)

- [ ] 1.1 The record: `ViewerAnnotation` gets `number`, `status`, `sent` and `replies` (`AnnotationReply` with `id`, `author`, `body`, `sent`, `from`); kinds `comment` and `change`, `question` and `delete` read as `comment` and as `change` with an empty replacement and never written; `ViewerRecord` gets `lastAnnotationNumber` and `reviewWait`, which `state.json` never holds. Serde names as in `docs/protocol.ts`. Unit tests for the wire shape and the legacy kinds in the mux module
- [ ] 1.2 Validation and reducer of `put_annotations` (no `number` or `replies` accepted, a change's replacement may be empty, a general annotation is a comment, numbering from the counter, replies and number kept on replace), `remove_annotations` (replies go with it), `put_replies` (author `user` only, an agent reply's ID refused, 50 per thread), `remove_replies`, `mark_sent` (1 to 5,100 IDs, agent reply IDs ignored); the byte limits now count replies. Unit tests ("A question on a passage", "A suggested deletion", "A general comment", "Numbers are not given twice", "One annotation too many", "A terminal pane", "Two windows add at the same time", "An edit after sending", "The user answers an agent", "An agent's reply is not edited", "Resolved after the agent's answer", "Reopened")
- [ ] 1.3 Trusted operations without a wire form: an agent's reply (server-chosen ID, `author` `agent`, `from`), the start and the end of a wait; each bumps the screen revision. Unit tests that a client op of these kinds does not deserialize and that an agent reply survives a replacement of its annotation ("An agent replies while the user edits", "No client writes an agent's reply")
- [ ] 1.4 `open_viewer` with `replace`: allowed only with a `text` source and without `newPaneId`; the first pane in layout order with the same title and origin gets the text and a raised revision, annotations kept; otherwise a new pane. Unit tests ("A revised plan", "Another title")
- [ ] 1.5 Persistence: threads, numbers, statuses and the counter in `state.json`, no `reviewWait`; a restore brings the threads back. Unit test in `daemon` ("Daemon restart", "Annotations persisted", "A wait is not persisted" for the record)

## 2. The server (`splitty-server`)

- [ ] 2.1 Mux handler: nudges and `mux.op_failed` for `put_replies`, `remove_replies` and `mark_sent`; nudges to every client after an agent reply and a wait's start and end; `viewer.review_answer` settles a wait once and is answered with `viewer.review_answered` to its sender. In-process tests beside the annotation tests in `client_session::mux_handler` ("Rejected reply", "Reply of an agent", "The first answer", "A stale answer", "Two windows answer")
- [ ] 2.2 Control API: `POST /api/mux/op` refuses the three new operations; `GET /api/viewer/comments` (screen scope, handed-over entries only, `document` as path or pane ID, `all`, `ambiguous`), `POST /api/viewer/comments/reply` (validation, limits, `from` with the pane's title as in D6). In-process router tests beside `http/tests/annotations.rs` ("Annotations refused", "Annotations only through the window", "Comments of a screen", "A reply", "Unknown number", "An agent reads a sent review", "A reply to an annotation not sent", "Another screen", "An agent replies")
- [ ] 2.3 Waits: `POST /api/viewer/review/wait` holds the request in a registry (one per viewer pane, 16 per server), ends with the first accepted answer, `closed` when the pane closes, and without an outcome when the request is dropped. In-process tests with the router and a fake answering client ("Approved", "Second wait", "Caller gone", "The agent stops waiting")
- [ ] 2.4 Routes: the three endpoints on the owner control socket and the HTTP listener, 404 on grant sockets. Extend the route-set tests ("Comments on the owner socket")

## 3. The CLI (`splitty-server`, `cli`)

- [ ] 3.1 `splitty comments [<document>] [--all] [--json]` and `splitty comments reply <document> <number> <text|->`, with `--pane`/`--screen` and the environment, the Markdown listing, refused under `SPLITTY_AGENT_SOCK`, the update hint for old servers. Unit tests for argument handling and the listing's Markdown; a smoke test in `tests/http_surface.rs` for one listing and one reply against a real `splitty` process, since the CLI's discovery of the server and its output are only visible there ("Read a sent review", "Reply", "Nothing sent yet", "Two viewers of one file", "Under a grant")
- [ ] 3.2 `splitty review <path>` and `splitty review --text [--title]`, the wait, stdout and exit status per outcome, `--json`, the message on standard error. Unit tests for the outcome handling; the smoke test of 3.1 extended by a review that a test client answers over the WebSocket ("A plan reviewed", "The next revision of the plan", "Approved", "Outcome as JSON", "The pane closed")
- [ ] 3.3 The agent guide (`crates/splitty-server/src/cli/agent_guide.md`): reading and replying to comments, waiting for a review

## 4. The window (`frontend/`)

- [ ] 4.1 Protocol: the new fields in the viewer record of `lib/mux/incoming.ts` (bounded, validated, malformed entries dropped), the new operations, `viewer.review_answer` and `viewer.review_answered`. Vitest in `lib/mux`
- [ ] 4.2 The store (`lib/viewer/annotations.ts`): replies, statuses, `sent`, the new operations and their settle rule, the pending entries and the review selection. Vitest ("An older snapshot arrives", "An agent replies while the user edits", "Sent again", "Added while sending")
- [ ] 4.3 Anchoring (`lib/viewer/anchoring.ts`): the similarity step and the `changed` state. Vitest ("The passage reworded", "The passage removed", "Lines inserted above the passage", "The same words twice")
- [ ] 4.4 The review (`lib/viewer/review.ts`): numbers, replies with authors, deletions, changed passages, the reply line with the path or the pane ID. Vitest ("A plan with a question and a suggested change", "A thread with an answer", "A terminal selection", "A quote with an escape sequence")
- [ ] 4.5 Making and editing: two kinds in the toolbar and the context menu, the change form with the deletion reading, Move to Selection. Vitest in a DOM environment ("Moved to another passage")
- [ ] 4.6 The thread list: replies and the reply box, Resolve and Reopen, the folded resolved group, sent markers, agent replies as plain text with their title, the changed and not-found states, the tab marker counting open annotations. Vitest ("Resolved after the agent's answer", "Reopened")
- [ ] 4.7 Marks: open annotations only, the changed style, Hide Marks and Show Resolved per pane in the view state. Vitest ("Marks hidden", "From the rendered view to the source")
- [ ] 4.8 Hand-over: `mark_sent` after a paste, a file or a scratchpad append, Send Again, copy and save marking nothing, the undo removed. Vitest with a fake terminal target ("Send to the agent", "Undo after sending", "Copied", "Into the scratchpad", "A program without bracketed paste", "The origin is gone")
- [ ] 4.9 The waiting banner: Approve and Send Review, `viewer.review_answer`, marking after `accepted`. Vitest ("Feedback on a plan", "Approved", "Two windows answer")
- [ ] 4.10 Check in a real browser against a local `serve` (Playwright, one-off script): threads with replies, resolving and the folded group, hidden marks, a reworded passage marked as changed, Move to Selection, a review sent twice (pending, then Send Again), an agent's reply through `splitty comments reply` appearing in the thread, `splitty review --text` answered with Approve and with a review

## 5. Documentation

- [ ] 5.1 `docs/protocol.ts` (the record, the operations, the messages, the endpoints), the agent guide, `docs/files-and-editors.md` (threads, sending, agents replying, `splitty review`), `docs/architecture.md` (section Viewer Panes), README

## 6. Verification

- [ ] 6.1 Targeted: `cargo test -p splitty-core mux::`, `cargo test -p splitty-core daemon::`, the new tests of `splitty-server`, `pnpm test` for the touched files in `frontend/`
- [ ] 6.2 Security review of the agent channel (scope, authorship, what a draft reveals, the held requests) and of how agent text is shown; findings fixed or recorded
- [ ] 6.3 Once before the merge: `cargo fmt --all --check`, `cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings`, `cargo test --workspace --exclude splitty-gui`; in `frontend/`, `pnpm check`, `pnpm test` and `pnpm build`; `openspec validate --specs`. `crates/splitty-tauri` stays untouched
