# Proposal

## Why

The review a viewer pane hands to an agent, and the place of each annotation in the current document, are worked out twice: once in the web window and once in the native window, with different results. The same annotation reads differently in a review depending on the window that sent it (the note on a changed passage, the number, the quoting of the pane ID, which entries a re-sent review marks), and the two anchoring algorithms measure in different units, treat different characters as whitespace and as scripts without spaces, and bound their searches differently, so one annotation can be anchored, changed or lost depending on the window. The server already holds and numbers the annotations and reads the documents; it should also build the review and anchor the annotations in the document's source, so that every window shows and sends the same.

## What Changes

- The server builds a viewer pane's review on request (`viewer.review` → `viewer.reviewed`): the pending annotations as it holds them after the connection's earlier messages, anchored in the document version it reads, with the verdict and the note of a document that changed during a wait when the window asks for them, and the user's entries the review holds, as `mark_sent` names them.
- The server anchors a pane's annotations in its document's source on request (`viewer.anchor` → `viewer.anchored`): for each annotation its state (`anchored`, `changed`, `lost`, or `general` for a general comment), its range in the source and its current lines, for the document version it names, in the order of the pane's list.
- One anchoring rule set on the server, the union of the two windows' cases, with one unit and one set of limits.
- A window that anchors annotations taken from its rendered view in that view's text (the web window's rendered Markdown and HTML) passes their state and lines with the review request; the server uses them for those annotations only.
- Both windows ask the server for the review and for source anchors and delete their own review builders and source anchoring; they keep the mapping of a source range into what they draw, the description of a selected passage, and the hand-over mechanics.
- New capability `viewer-reviews-v1` in `mux.welcome`. A window offers annotations, reviews and verdicts only on a server that announces it; on an older server it says that the host needs an update.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `viewer-annotations`: Anchoring By Quote (the server anchors in the source, a window in its rendered view), The Review (the server builds it; wording decisions), Annotations Capability (`viewer-reviews-v1`, older host).
- `mux-protocol`: a new requirement for the review and anchor messages on the session connection.

## Impact

- Server: a pure review and anchoring module in the core's mux, `viewer.anchor` / `viewer.review` handled beside `viewer.read` on the session connection, the capability in the welcome, the session operation catalog (`docs/native-operations.json`).
- Native client: client-core's annotation module loses its anchoring and review text; the worker sends the two requests and reports their answers; the window anchors and reviews through them.
- Web client: `lib/viewer/review.ts` loses its text builder; the annotation layer takes source anchors from the server and keeps its rendered-view anchoring.
- Docs: `docs/protocol.ts`, `docs/architecture.md` (viewer sections), `docs/native-ui-handoff.md`.

## Out of scope

- Describing a selected passage as a target (quote, context, lines): stays in each window, which knows its selection.
- Anchoring in the text of a rendered view: only the window has that text; the web window keeps its anchoring for it.
- The file name a review is stored under, the hand-over policy (paste, file, scratchpad) and the labels that say whether a review holds the pending annotations or all open ones.
- The CLI's own Markdown of `splitty comments`.
