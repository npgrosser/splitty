# Design

## Context

`review-threads` added waits for a review: `splitty review` shows a document in a viewer pane and holds `POST /api/viewer/review/wait` until a window answers with `viewer.review_answer`, either `approved` without a review or `feedback` with one. The banner of the waiting pane offers Approve and Send Review; Send Review needs an open annotation. The review lists the comments on passages in the order of the pane's list and the general comments last, under `## General comments`.

Three gaps remain. A review carries no decision, so the waiting agent cannot tell "go ahead, but note this" from "revise this", and a script cannot branch on the exit status. The document as a whole has no prominent place for the user's answer: a general comment is made from the list and lands at the end of the review. And a wait is visible only in its pane.

## Goals / Non-Goals

**Goals:**
- Every answer to a wait states a verdict, for the agent in the review's text and for scripts in the exit status and the JSON outcome.
- A rejection without any comment is possible, and a written answer to the whole document is one field away while a process waits.
- General comments read first, in the review and in the list.
- The user notices a waiting process on the pane's tab, and the review notes a document that changed while it was reviewed.

**Non-Goals:**
- Verdicts for reviews handed to a terminal, a third verdict, a version pin on the server, notifications outside the window, and the plan-approval hook of an agent (see the proposal).

## Decisions

### D1. Two verdicts (provisional, tbd)

The banner offers **Approve** and **Request Changes**.
- **Approve** answers `approved`. When the pane holds pending entries (the review's selection of `review-threads` D4) or the user wrote an overall comment, the answer carries a review of the pending entries as notes. Otherwise it carries none, as before. Entries that were handed over before are not repeated: an approval adds nothing to them.
- **Request Changes** answers `changes_requested` with a review. It holds the pending entries, or every open one when none is pending, as Send and Send Again do. It may hold no annotation at all, which is a plain rejection.

Both buttons are always offered while the wait is open; the former rule that Send Review needs an open annotation goes. A third verdict that discards the document is left out: a rejection needs to say what to do instead, and that is a request for changes with a general comment.

Alternative: keep Send Review as a third, verdict-free answer. That brings back the ambiguity the verdict removes.

### D2. Exit status 3 for a request for changes (provisional, tbd)

`splitty review` exits 0 on an approval, 3 when the user requests changes, 1 when it fails and 2 on a usage error, with or without `--json`. A plain approval prints nothing; an approval with notes and a request for changes print the review. The code 3 is free in the CLI (1 and 2 are taken across commands), so `until splitty review …; do …; done` stops on a failure only by checking for 3, and `splitty review … && …` goes on only after an approval.

Alternative: exit 0 for both verdicts and tell them apart by the output. That leaves scripts to parse the review.

### D3. The overall comment is a general comment (provisional, tbd)

The banner holds a text field, "Overall comment (optional)". When the user answers with text in it, the window first saves the text as a new general comment, waits until the server has given it a number, and then builds the review, which holds it as a pending entry. The comment is a thread like any other: it is marked as handed over with the answer, and an agent can reply to it. The window clears the field once the comment is saved. When the wait ends before an answer, the window keeps the field's text for the pane and offers it with the next wait.

Alternative: a verdict message carried only in the answer. It would be a second kind of general text, without a number and without a thread.

### D4. The review format (provisional, tbd)

The review of an answer to a wait states its verdict right after the heading and the document's path, and every review lists the general comments before the comments on passages:

```markdown
# Review of Plan

Verdict: changes requested

Note: the document changed during the review; lines and quotes refer to the version shown when the review was answered.

## General comments

- #4 Split the migration into its own step.

## #1 Line 3, comment
> with a TTL of a minute

Make the TTL configurable.

Reply in a thread: splitty comments reply viewer-0123456789abcdef <number> '<text>'
```

The verdict line reads `Verdict: approved` or `Verdict: changes requested`. The note line appears only when the window saw the document change during the wait (D6). A review handed to a terminal carries neither. A review that holds no annotation, which only a plain rejection produces, leaves out the reply line, since it has no thread to reply in.

### D5. General comments first in the list (provisional, tbd)

The pane's list shows the open general comments first, in creation order, then the anchored annotations in document order, then those not found, and the resolved ones folded after them. The review follows the list, so the order is the same in both places.

Alternative: general comments first in the review only. The list and the review would then disagree, and the review's order would need its own rule.

### D6. A changed document, noted by the window (provisional, tbd)

When a window first shows a wait, it records the version of the document it shows; while the wait stays open and the window shows another version, the banner says that the document changed since the review began, and the review of the answer carries the note of D4. A text document cannot change during a wait, since the server refuses to replace it (`viewer-panes`, Reopening A File), so the note concerns files. A window that opens the pane later records the version it first shows.

Alternative: the server records the file's version when the wait starts, and the answer names the version it judged. That needs the server to read the file at the start of every wait, for a case that is rare while the agent itself waits.

### D7. A marker for a waiting review on the tab

While a viewer pane's record names a wait, the pane's tab shows a marker whose tooltip names the waiting pane's title; a click on it makes the pane active. Nothing crosses the protocol: the record already names the wait.

### D8. No alias for `feedback`

The outcome `feedback` has not been released, so it is removed instead of kept as an alias: an answer with it breaks the rules and settles nothing.

## Wire shapes

`viewer.review_answer` (client → server):

```json
{"type": "viewer.review_answer", "screenId": "s1", "paneId": "viewer-0123456789abcdef", "waitId": "wait-0123456789abcdef",
 "outcome": "approved" | "changes_requested", "review": "…"}
```

`review` is optional with `approved` and required with `changes_requested`; when present it is 1 byte to 1 MiB of UTF-8. Every other combination breaks the rules.

`POST /api/viewer/review/wait` responds 200 with one of:

```json
{"outcome": "approved"}
{"outcome": "approved", "review": "…"}
{"outcome": "changes_requested", "review": "…"}
```

`splitty review --json` prints the same objects.

## Frontend structure

- `review_banner.ts`: the field, the two buttons, the note on a changed document.
- `annotation_layer.ts`: the answer per verdict, the overall comment saved before the review, the field's text kept per pane, the version recorded per wait.
- `lib/viewer/review.ts`: the verdict and note lines, general comments first, no reply line in a review without annotations; `compareListPlaces` puts general comments first.
- `lib/viewer/annotations.ts`: the wait of each viewer pane, from the snapshots, for the tab.
- `layout/tab_renderer.ts`: the marker.

## Risks / Trade-offs

- [An agent may read exit status 3 as a failure of the command] → The agent guide says that 3 is the user's request for changes and that the review follows on standard output; the review starts with its verdict line.
- [Approving with notes marks them as handed over, so a later Send does not repeat them] → That is the meaning of handed over; Send Again still holds every open annotation.
- [The overall comment is saved before the answer; when the answer then fails, the comment stays pending in the list] → It is an ordinary general comment, sent with the next review.

## Migration

None: the outcome `feedback` and the banner's Send Review have not been released.

## Architecture impact

None. No component or relationship of `architecture/*.c4` changes, and the concept sections of `docs/architecture.md` stay as they are.

## Provisional decisions

Decided for this change, marked in `openspec/open-decisions.md` (2026-11-06):

- **D1**: two verdicts; Approve carries pending entries as notes; Request Changes may hold no annotation.
- **D2**: exit status 3 for a request for changes.
- **D3**: the overall comment is a general comment, saved before the answer.
- **D4**: the verdict and note lines, general comments first, no reply line without annotations.
- **D5**: general comments first in the list.
- **D6**: a changed document noted by the window.

## Open Questions

- Should an approval also resolve the annotations it carries as notes? For now nothing changes their status.
