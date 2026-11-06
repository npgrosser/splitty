# Design

## Context

`review-pane` gave viewer panes annotations: four kinds, kept in the viewer record, anchored in the window by quote, handed to a terminal as a Markdown review, and removed when the review is sent, with a ten-second undo. The control API neither shows nor accepts them. In use that falls short in three ways: a review leaves nothing behind, so a follow-up on the same passage starts from scratch; the agent answers in the terminal, away from the passage; and a harness that wants the user's verdict on a plan before it goes on (the plan-mode hook pattern) has nothing to wait on. The kinds `question` and `delete` add a choice that the remark itself already makes.

This change turns annotations into threads that stay, lets an agent read what the user handed over and answer in the thread, and adds a command that waits for the user's verdict. It changes data that has not been released: `viewer-annotations-v1` is redefined before its first release.

## Goals / Non-Goals

**Goals:**
- Threads that stay: replies, a number per pane, open or resolved, handed-over marks instead of removal.
- An agent can read what the user handed over and reply, without seeing drafts and without writing anything but its own replies.
- `splitty review` blocks until the user approves a document or sends a review.
- A passage that an agent reworded keeps its thread, marked as changed; a thread can be moved.
- Marks can be hidden, resolved threads fold away.

**Non-Goals:**
- Comments bound to a file across viewer panes; editing documents; an MCP server; timestamps; notifications outside the window; threads an agent starts; grant-socket access.

## Decisions

### D1. Two kinds (provisional, tbd)

`comment` and `change`. A `change` carries a replacement that may be empty, which suggests deleting the passage; a general comment (no target) is a `comment`. A question is a comment whose thread the agent answers. The toolbar and the context menu offer **Comment** and **Suggest Change**; the change form starts with the passage as replacement, and clearing it reads as a deletion.

Alternative: keep four kinds. They cost a choice per remark and change only a heading.

State from builds before this change may hold `question` and `delete`. Since a `state.json` with a record the server cannot read is dropped as a whole (see `docs/issues/openspec-baseline-findings.md`), the server reads `question` as `comment` and `delete` as `change` with an empty replacement, wherever an annotation is deserialized. It never writes them.

### D2. Threads, and who may write what (provisional, tbd)

An annotation holds `replies` in the order they were added, at most 50. A reply is `{id: "rep-<16 hex>", author: "user" | "agent", body, sent?, from?}`.

- The window writes the annotation itself (`put_annotations`, `remove_annotations`), the user's replies (`put_replies`), removes replies of either author (`remove_replies`) and marks entries as handed over (`mark_sent`). Status (`resolved`) is a field of the annotation the window puts.
- An agent adds replies only, through the control API; the server chooses the ID, sets `author: "agent"` and `from: {paneId, title}` from the caller, and never accepts `author` or `from` from a client. A window cannot put a reply under an agent reply's ID, so an agent's text stays as written; the user can remove it.
- `put_annotations` carries no `number` and no `replies`; replacing an annotation keeps both, so an agent reply that lands while the user edits survives.

Alternative: replies as part of `put_annotations` (one write path, but last write wins would drop concurrent agent replies), or agents allowed to resolve (the user would lose the overview of what is settled).

### D3. Numbers per pane (provisional, tbd)

The server numbers annotations per pane in the order it adds them, from `lastAnnotationNumber + 1`, and keeps the counter in the viewer record, so a removed number is not given again. The number is the handle the list, the review and the CLI share (`#3`). Numbers per pane read naturally in a review of one document; the CLI names the document beside the number.

Alternative: numbers per workspace (`splitty comments reply 17 ...` without a document), which reads oddly in a review that starts at #17.

### D4. Sending marks instead of removing (provisional, tbd)

An entry of the user (the annotation itself or a reply) is pending until it has been handed over. The review holds the open annotations with a pending entry, each with its whole thread for context; when none is pending, it holds all open annotations, and the main action reads **Send Again**. Sending to a terminal, as a file or into the scratchpad, and answering a waiting process with a review, mark the review's entries with `mark_sent`. Copying and saving mark nothing. A window that changes a handed-over entry puts it without `sent`, so it is pending again. There is no undo, since nothing is removed.

`mark_sent` takes IDs rather than whole annotations, so it cannot overwrite an edit another window made in the meantime. It ignores IDs of agents' replies.

### D5. What an agent sees (provisional, tbd)

Through the control API, with the caller's screen and pane, an agent reads the annotations of that screen's viewer panes whose own entry was handed over, with the agents' replies and the user's replies that were handed over. A draft never leaves the window. The screen scope keeps an agent in one project from reading another's review. It is no boundary against a process of the same user that names another screen or reads the token file, and the design does not claim one; the boundary that holds is that grant sockets get none of this.

Both control transports serve the endpoints, like the editor and file-offer requests; the owner socket lists them in its route set.

### D6. The title of an agent's reply

`from.title` is the title the server knows for the caller's pane when the reply arrives: the pane's own title if it has one, else its session's process title, else its OSC title, else `agent`; at most 256 bytes. It names the reply in the window and in reviews ("Agent (claude)").

### D7. Similarity anchoring (provisional, tbd)

After the exact and the whitespace-tolerant search, the window looks for the passage the quote became: candidate spans of the current text that start within 50 lines of the recorded start line and are between half and twice the quote's length, scored by the Dice coefficient of their character trigrams after whitespace is collapsed and case is folded; the best candidate with a score of at least 0.6 anchors the annotation as `changed`. Quotes over 4,000 characters skip this step. A changed annotation is marked in the document in its own style and listed with its original quote; the review notes "passage changed since". **Move to Selection** gives an annotation the current selection as its new target (an ordinary `put_annotations`), which also ends a `changed` or `not found` state.

Alternative: a diff-based mapping of old to new text. It needs the old text, which the window does not keep across reloads.

### D8. The review format (provisional, tbd)

````
# Review of PLAN.md

Document: /home/me/app/PLAN.md

## #3 Lines 12-14, comment
> Cache the session list and invalidate it
> on every change.

Why not compute it on demand? The list has at most 20 entries.

Agent (claude): The list is shared by two windows, so it is cached per connection.

User: Then compute it per connection.

## #5 Line 30, suggested change
> retry three times

Replace with:
```
retry with exponential backoff, at most five times
```

## #6 Line 41, suggested deletion
> The old migration path.

## General comments

- #7 Split step 4 into two steps.

Reply in a thread: splitty comments reply /home/me/app/PLAN.md <number> "<text>"
````

The last line names the document as the CLI takes it: the path for a file, the viewer pane's ID for a text. It reverses `review-pane`'s rule that a review carries no instruction, because the reply path did not exist then; it stays one line.

### D9. Waiting for a review (provisional, tbd)

`splitty review` opens the document as `splitty open` does (for `--text`, with `replace`, see D10), then posts `POST /api/viewer/review/wait` and holds the request. The server keeps waits in memory, puts `reviewWait: {id, paneId, title}` into the viewer record with a trusted operation, and nudges the screen. A window that shows the pane says that `<title>` waits for a review and offers **Approve** and **Send Review**. It answers with `viewer.review_answer`; the server settles the held request with the first answer, ends the wait with another trusted operation, and acknowledges every answer with `viewer.review_answered`. A window marks the review's entries only after `accepted: true`. A closed pane ends the wait with `closed`; a dropped request (Ctrl+C, a killed process) ends it without an outcome. The record field is never written to `state.json`, and a restarted daemon has no waits.

The command prints the review and exits 0 on feedback, prints nothing and exits 0 on approval, and exits 1 when the review closes without an answer. A hook script tells approval from feedback by the empty output; `--json` names the outcome.

Alternative: exit codes per outcome. They collide with usage errors, which exit 2.

### D10. Replacing a text document (provisional, tbd)

`open_viewer` gets `replace: true`, allowed only with a `text` source and without `newPaneId`: the first viewer pane of the screen that shows a text with the same title and the same origin gets the new text and a raised revision, and keeps its annotations, which the window anchors again in the new text. A plan reviewed in rounds keeps its threads, and the reworded passages show as changed (D7). The window's own "Open Selection in Viewer" never sets it.

### D11. Threads stay with the pane (provisional, tbd)

Annotations still live and end with their viewer pane. With threads that are never removed by sending, closing the pane is the user's "done", and it asks first. `splitty open` and `splitty review` reuse the pane of the same file or titled text, so the normal loop keeps one pane.

Alternative: annotations bound to the file on the server, shown again in any viewer of it. That needs a store outside the screen tree, a cleanup rule and a decision about files that moved; it is left for later.

### D12. Limits (provisional, tbd)

As before: 100 annotations and 262,144 bytes of text per pane, now including replies; 16,384 bytes per remark, replacement, quote and reply; 50 replies per thread; 256 bytes for a reply's title; `mark_sent` takes up to 5,100 IDs (100 threads of 51 entries); a review answer up to 1 MiB; 16 waits per server, one per viewer pane.

### D13. No new capability

`viewer-annotations-v1` has not been released; it now means the threads, the new operations and the endpoints. A window offers replies, resolving, sending marks and the waiting banner only with it, as it offered annotations before.

## Wire shapes

These go into `docs/protocol.ts`; the Rust types and `frontend/src/lib/protocol/types.ts` follow them.

```ts
interface ViewerRecord {
  // ... as before
  annotations?: ViewerAnnotation[];   // creation order, absent while empty
  lastAnnotationNumber?: number;      // highest number given, absent while 0
  reviewWait?: ReviewWait;            // never persisted
}

interface ViewerAnnotation {
  id: string;                         // "ann-" + 16 lowercase hex, chosen by the window
  number?: number;                    // set by the server; never in put_annotations
  kind: "comment" | "change";
  body?: string;                      // a comment needs one that is not only whitespace
  replacement?: string;               // a change needs one; "" suggests deleting the passage
  target?: AnnotationTarget;          // absent for a general comment, which is a comment
  status?: "resolved";                // absent: open
  sent?: true;                        // the user handed the annotation itself over
  replies?: AnnotationReply[];        // server-kept, absent while none; never in put_annotations
}

interface AnnotationReply {
  id: string;                         // "rep-" + 16 lowercase hex
  author: "user" | "agent";
  body: string;                       // 1..16,384 bytes, not only whitespace
  sent?: true;                        // user replies only
  from?: { paneId: string; title: string };  // agent replies only
}

interface ReviewWait { id: string /* "wait-" + 16 hex */; paneId: string; title: string; }

// Ops, window only (POST /api/mux/op answers 400):
{ type: "put_annotations"; paneId; annotations: ViewerAnnotation[] /* 1..100, no number/replies */ }
{ type: "remove_annotations"; paneId; ids: string[] /* 1..100 */ }
{ type: "put_replies"; paneId; annotationId: string; replies: { id; author: "user"; body; sent? }[] /* 1..50 */ }
{ type: "remove_replies"; paneId; annotationId: string; ids: string[] /* 1..50 */ }
{ type: "mark_sent"; paneId; ids: string[] /* 1..5,100 ann-/rep- IDs */ }
// open_viewer gains: replace?: true   (text source, no newPaneId)

// Messages
// client -> server
{ type: "viewer.review_answer"; screenId; paneId; waitId; outcome: "approved" | "feedback"; review?: string /* feedback only, <= 1 MiB */ }
// server -> that client only
{ type: "viewer.review_answered"; paneId; waitId; accepted: boolean }

// Control API (both transports; not on grant sockets)
GET  /api/viewer/comments?screenId=&paneId=[&document=][&all=true]
  200 { documents: { paneId; source: { kind: "file"; path } | { kind: "text"; title }; annotations: {
          number; kind; status: "open" | "resolved"; body?; replacement?;
          target?: { view; quote; startLine; endLine };
          replies: { author: "user" | "agent"; body; from?: { paneId; title } }[] }[] }[] }
POST /api/viewer/comments/reply { screenId; paneId; document; number; body }
  200 { ok: true; paneId; number }
POST /api/viewer/review/wait { screenId; paneId; viewerPaneId }
  200 { outcome: "approved" } | { outcome: "feedback"; review: string }
// errors: { code, error } with 400 invalid, 404 not_found, 409 ambiguous | busy | limit, 410 closed, 429 busy
```

## Behavior details

- `document` resolves among the screen's viewer panes: a value starting with `viewer-` names a pane; otherwise it is an absolute path matched against `file` sources. The CLI makes a path absolute against its directory, following symbolic links, before it sends it.
- A reply needs the annotation's own `sent`. Replies to resolved annotations are accepted and do not reopen them.
- The comments listing sorts documents in layout order and annotations by number.
- The trusted operations (agent reply, wait start, wait end) run through the same apply path as other operations, so they persist (the reply), nudge and bump the screen revision; they have no wire form a client could submit.
- `splitty comments` without `--json` prints the listing as Markdown in the form of D8, without the last line, with the lines the annotations had when they were made.
- The waiting banner offers Approve also when annotations are pending; approving then keeps them pending.

## Frontend structure

- `lib/viewer/annotations.ts` (the store per connection): replies, statuses, `sent`, the new operations with the same settle rule as before; `pendingEntries` and the review selection (D4).
- `lib/viewer/anchoring.ts`: the similarity step (D7) and the `changed` state.
- `lib/viewer/review.ts`: D8, with the authors of replies.
- `lib/viewer/handover.ts`: marking instead of removing; answering a wait.
- `panes/viewer/selection_toolbar.ts`, `annotation_form.ts`: two kinds, the deletion reading, Move to Selection.
- `panes/viewer/annotation_list.ts`: threads with replies, a reply box, Resolve and Reopen, the resolved group, sent markers, agent replies as plain text with their title, the changed and not-found states.
- `panes/viewer/annotation_layer.ts`, `lib/viewer/source_view.ts`: marks for open annotations, the changed style, the hide and show-resolved choices (kept in `panes/viewer_view_state.ts`).
- `panes/viewer_view.ts`: the waiting banner.

## Risks / Trade-offs

- [An agent reads a review the user sent to another terminal of the screen] → screen scope is the rule, by design; what is read was handed over.
- [A process names a screen or pane that is not its own] → it reads what was handed over there; the same user's process could read the token anyway. Grant sockets stay out.
- [An agent floods a thread] → 50 replies per thread and the pane's byte limit; the user can remove replies.
- [Prompt injection through an agent's reply into the next review] → the reply goes back to the agent that wrote it, or one the user chooses; it is shown as plain text and never sent on its own.
- [Similarity anchors at the wrong passage] → only near the recorded lines and above the threshold, marked as changed, with the original quote shown; Move to Selection corrects it.
- [A held review request] → one per pane, 16 per server, ended by a closed pane or a dropped connection, never persisted.
- [Hidden text in a quote], as in `review-pane`, remains; the review still goes to the prompt without Enter.

## Migration

None for released data. Unreleased state with `question` or `delete` is read as described in D1. An older window ignores `replies`, `number`, `status`, `sent` and `reviewWait`; it would drop sent annotations on its next send, which only matters for pre-release builds.

## Architecture impact

No new crate, top-level module or relationship between components: the record, the operations and the trusted operations extend `splitty-core`'s `mux`; the server's mux handler, control API, routes and CLI extend what they do for viewer panes and file offers. `docs/architecture.md`'s section "Viewer Panes" describes threads, the agent channel and waits.

## Provisional decisions

Decided for this change, marked in `openspec/open-decisions.md` (2026-11-06):

- **D1**: two kinds; a change with an empty replacement is a deletion.
- **D2**: agents only add replies; replies are separate from `put_annotations`.
- **D3**: numbers per pane, never given twice.
- **D4**: sending marks instead of removing; the pending selection and Send Again.
- **D5**: agents read handed-over entries of their screen.
- **D7**: similarity anchoring with its parameters.
- **D8**: the review format with the reply line.
- **D9**: `splitty review` and its outcomes.
- **D10**: replacing a text by title and origin.
- **D11**: threads stay with the pane.
- **D12**: the limits.

## Open Questions

- Comments bound to a file (D11's alternative), and with it a history across panes.
- An MCP surface (roadmap 104) over the same endpoints.
- Telling the agent when the user replied, beyond the next review.
