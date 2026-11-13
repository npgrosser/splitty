# Design

## Context

The server keeps a viewer pane's annotations in the pane record, numbers them, validates them, and reads the pane's document for `viewer.read` (a file of at most 8 MiB opened anew, or the text in the record). It never interprets the annotations' text. The web window (`lib/viewer/review.ts`, `lib/viewer/anchoring.ts`, `panes/viewer/annotation_layer.ts`) and the native window (client-core `annotations`, gpui `workspace/viewer/notes.rs`) each anchor the annotations and write the review. The native window anchors every annotation in the source and maps the source range into its rendered Markdown view; the web window anchors an annotation taken from its rendered view in the rendered view's DOM text, which only the browser has, and every other one in the source.

The two implementations differ (audit, verified in the code):

- review text: `(passage changed since)` (web) and `(passage changed in the current version)` (native); `#N` only when the window knows the number (web) or always (native); the pane ID quoted by the upload-path rule (web) or a shell-word rule (native); re-sent reviews mark only pending entries (web) or every held entry (native, a no-op for entries already marked);
- anchoring: offsets in UTF-16 units (web) and UTF-8 bytes (native); context, quote and region limits in UTF-16 units (web) and characters or bytes (native); whitespace is JavaScript's `\s` (with U+FEFF, without U+0085) or Unicode White_Space (with U+0085, without U+FEFF); scripts without spaces include Lao, Khmer and Myanmar only on the web; only the web caps occurrences (100,000) and the similarity work per document version (4,000,000 steps); the whitespace-collapsed search compares collapsed context (web) or the original context (native); changed passages may run past the search region (web) or not (native); ties between equally similar spans go to different spans.

## Goals / Non-Goals

**Goals:**

- One review text and one source anchoring, on the server, the union of both windows' cases.
- Windows only render: map a source range into their view, describe a selection, hand the text over.
- Each step of the migration leaves a working tree.

**Non-Goals:**

- Anchoring in a rendered view's text on the server (the server has no rendered text; see SBR-5).
- Moving target description, list labels or the hand-over policy.

## Decisions

### SBR-1 Two session messages beside `viewer.read`

`viewer.anchor {requestId, paneId}` → `viewer.anchored {requestId, paneId, version, anchors}` and `viewer.review {requestId, paneId, verdict?, changedDuringWait?, passages?}` → `viewer.reviewed {requestId, paneId, version, text, count, entries}`; failures are `viewer.failed {requestId, code, error}` with the codes of `viewer.read`. They are handled by the same component that answers `viewer.read`, off the connection's message loop, sharing its four reads in flight. The pane's annotations and source are taken from the store when the message is processed, so that every op the connection sent before is in them; then the document is read whole (as `viewer.read` would, offset 0) and anchored. Alternative: answer anchors in the screen snapshot. Rejected: the server does not follow file versions, and snapshots go to every window.

Native carriers list both in the session operation catalog. A hub relays them like `viewer.read`, so a host's pane is anchored and reviewed by its host.

### SBR-2 Review text (provisional, tbd)

The server writes `#N` always (it holds every number); the note on a changed passage is `(passage changed in the current version)`, parallel to `(passage not found in the current version)`; the pane ID in the reply line is bare when it consists of letters, digits and `-_./:`, else in POSIX single quotes. The entries a review names are only the user's entries it holds that are not handed over (the web window's rule; the native rule only added no-op marks). Everything else of the two builders agrees and is kept: heading, document line, verdict lines, wait note, general comments as a list, passage sections, fences, replies with indentation, the U+FFFD replacement set.

### SBR-3 One anchoring (provisional, tbd)

The server's anchoring follows the web window's algorithm, which is the larger case set, measured in Unicode scalar values:

- context: 32 characters on each side when a window describes a passage (unchanged), compared character by character;
- exact search over at most 100,000 occurrences, best context, then nearest line, then first;
- whitespace-collapsed search with collapsed context; whitespace is Unicode White_Space;
- changed passages: spans that start within 50 lines of the recorded start line and in at most 65,536 characters around it, may extend past that region, are between half and twice as long as the quote (whitespace collapsed, case folded), start and end at word boundaries or at any character of Han, Hiragana, Katakana, Thai, Lao, Khmer or Myanmar; scored by the Dice coefficient of character trigrams, at least 0.6; quotes over 4,000 characters skip this step; at most 1,000,000 span ends per annotation and 4,000,000 per document version; the best score wins, then the start nearest the recorded line, then the earlier start, then the shorter span;
- an empty quote is `lost`.

### SBR-4 Units of a range

`start` and `end` count UTF-8 bytes of the document's source text: its bytes decoded as UTF-8 with U+FFFD for invalid sequences, a leading byte order mark dropped, and every CRLF and lone CR as LF. That is what the web window's source view holds (`TextDecoder` drops the byte order mark; the view keeps line feeds only). The native window decodes its documents the same way, with the core's function, so the ranges index its text directly; the web window converts them to UTF-16 offsets of its source text. Lines are 1-based; a passage ends on the line of its last character, a trailing line feed not counted.

### SBR-5 Rendered-view annotations stay anchored by the window that has the view (provisional, tbd)

The server anchors every annotation in the source, also one taken from a rendered view (as the native window did). The web window anchors an annotation taken from its rendered view in the rendered text while it can render the document, and sends that result with `viewer.review` as `passages` (`{id, state, startLine, endLine, offset?, unlined?}`, `unlined` for a page that names no source lines); the server uses a passage only for an annotation that holds a rendered-view target and ignores passages of annotations it does not hold. Alternative: the window uploads its rendered text and line segments for the server to anchor in. Rejected for now: a round trip of up to the document's size on every version and view change, for the same result. Consequence: the web window keeps `lib/viewer/anchoring.ts` for its rendered view; its source-view annotations use the server's anchors.

### SBR-6 List order

`viewer.anchored` lists the anchors in the order of the pane's list: general comments in creation order, then anchored ones by start line, the source view before the rendered one on one line, then by offset in their view and end line, then those not found by their recorded lines, creation order breaking ties. The review uses the same order with the window's passages. The native window lists in that order; the web window, which mixes its rendered anchors in, keeps its comparator, which is the same rule.

### SBR-7 Versions

The server anchors the version it reads when the request is processed and names it. A window applies an answer only when it names the version the window shows and is the answer to its latest request; otherwise it waits for its next read of the document and asks again. A review is about the version the server read; the note of a document that changed during a wait stays the window's statement (`changedDuringWait`), since only the window knows which version it showed when the wait began.

### SBR-8 Older server (provisional, tbd)

A window offers annotations, the review and the verdicts only on a server that announces `viewer-reviews-v1`, and says on an older one that the host needs an update. A minimal local fallback would be a second review builder and a second anchoring, which this change removes. A process waiting on such a host can be interrupted; the window offers no verdict.

### SBR-9 Asynchronous anchors in the windows

Both windows ask for anchors when the document version or the annotations' targets change and draw the marks when the answer comes. Until then the anchors of the last answer for the same version stand for the annotations whose targets they were made for; any other annotation keeps no mark and is listed at its recorded lines without a state note. The review is asked for when the user sends, copies, saves or answers, and the action continues with the answer; the user's entries the answer names are marked as before.

## Architecture impact

- `splitty_core::mux` gains the pure review and anchoring (a submodule); no new component or relationship.
- `splitty_server::client_session` answers the two messages (existing relationship to `splitty_core::mux`).
- `splitty_client_core` loses its anchoring and review text and gains the two requests in its native workspace worker (existing relationships).
- Web frontend: `lib/viewer/review.ts` keeps only the file name and list comparator; a client of the two messages per connection.
- docs/architecture.md: the viewer annotations sections (server, web, native) change.

## Risks / Trade-offs

- [Marks arrive after a round trip] → the request is small and answered off the loop; an edit of a remark or a reply changes no target and asks for nothing.
- [A file changed between the window's read and the server's] → the answer names its version; the window waits for its own read of that version.
- [Rendered-view anchoring remains in the web window] → it is the only place with that text; the server's rule set is the same algorithm, so the two agree on the same text.

## Provisional decisions

SBR-2, SBR-3, SBR-5 and SBR-8 (2026-11-13), open to the maintainer's feedback; bullet in `openspec/open-decisions.md`.
