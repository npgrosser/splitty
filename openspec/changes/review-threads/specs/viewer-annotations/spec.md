## MODIFIED Requirements

### Requirement: Annotations On A Viewer Pane
A viewer pane (see `viewer-panes`) SHALL carry a list of annotations, empty when the pane opens; each annotation opens a thread (see Threads). Each annotation SHALL have an ID chosen by the window, of the form `ann-` followed by 16 lowercase hex characters, unique within its pane; a number the server gives it when it is added, one more than the highest number the pane has given, so that no number is given twice in a pane; a kind, `comment` or `change` (a suggested replacement of its passage, where an empty replacement suggests deleting the passage); and a status, open or resolved. An annotation SHALL either name a passage as its target, with the view it was taken from (`source` or `rendered`), the quoted text, up to 32 characters of context before and after it, and the source lines of the passage when it was annotated, or name no passage as a general comment, which SHALL be a `comment`. A `comment` SHALL carry a remark; a `change` SHALL carry a replacement, which MAY be empty, and MAY carry a remark. A pane SHALL hold at most 100 annotations and at most 262,144 bytes of text over all of them and their replies; a remark, a replacement, a quote and a reply SHALL be at most 16,384 bytes each and a context at most 256 bytes; lines SHALL lie between 1 and 10,000,000 with the first not after the last. The server SHALL refuse an operation that breaks these rules, or names a pane that is not a viewer pane, as a whole and without changing the workspace. A window SHALL offer annotations for documents shown as text, rendered or as source, and none for images and binary files.

#### Scenario: A question on a passage
- **WHEN** the user selects `retry three times` on line 30 of the source of `PLAN.md` and comments with a question
- **THEN** the pane holds one open annotation of kind `comment` with that quote, view `source`, lines 30 to 30, its context, the question as its remark and the pane's next number

#### Scenario: A suggested deletion
- **WHEN** the user suggests a change of a passage and empties the replacement
- **THEN** the pane holds an annotation of kind `change` with an empty replacement

#### Scenario: A general comment
- **WHEN** the user adds a comment without selecting a passage
- **THEN** the pane holds an annotation of kind `comment` without a target

#### Scenario: Numbers are not given twice
- **WHEN** a pane holds annotations #1 to #3, the user removes #3 and adds a comment
- **THEN** the new annotation has number 4

#### Scenario: One annotation too many
- **WHEN** a window puts a new annotation on a pane that holds 100 annotations
- **THEN** the server rejects the operation, and the pane keeps its 100 annotations

#### Scenario: A terminal pane
- **WHEN** a window puts an annotation on a terminal pane
- **THEN** the server rejects the operation, and the workspace stays unchanged

### Requirement: Saving Annotations Through The Window
A client SHALL make and change annotations over `mux-protocol` with these screen-level operations, each naming the viewer pane by `paneId`:
- `put_annotations` with 1 to 100 annotations, which carry no number and no replies. It SHALL replace an annotation with the same ID in its place, keeping its number and replies, and append a new one with the pane's next number.
- `remove_annotations` with 1 to 100 annotation IDs. It SHALL remove those annotations with their replies and ignore IDs the pane does not hold.
- `put_replies` with an annotation ID and 1 to 50 replies of the author `user`. It SHALL replace a reply of the user with the same ID in its place and append a new one. A reply ID that names an agent's reply SHALL be refused.
- `remove_replies` with an annotation ID and 1 to 50 reply IDs, of either author. It SHALL ignore IDs the annotation does not hold.
- `mark_sent` with 1 to 5,100 IDs of annotations and of the user's replies. It SHALL mark them as handed over to an agent and ignore IDs the pane does not hold.

An annotation and a reply of the user SHALL carry whether they have been handed over, as the window last put them, so that a window that changes an entry the user handed over puts it as not handed over. The server SHALL keep the annotations in the viewer record, which screen snapshots carry as `annotations` in creation order, absent while empty, each with its replies in the order they were added, and SHALL let the other connected clients know (see `mux-protocol`, Change Propagation to Other Clients).

#### Scenario: A second window sees the annotation
- **WHEN** window A adds an annotation to a viewer pane while window B shows the same pane
- **THEN** window B shows the annotation after its resync, and A receives nothing for its operation

#### Scenario: Two windows add at the same time
- **WHEN** windows A and B each add an annotation to the same pane before either has seen the other's
- **THEN** the pane holds both annotations, with different numbers

#### Scenario: An edit after sending
- **WHEN** the user edits the remark of an annotation that was handed over
- **THEN** the annotation is no longer marked as handed over, and the next review holds it

### Requirement: Concurrent Annotation Edits
Edits of one annotation, or of one reply, from several windows SHALL resolve as last write wins. Replacing an annotation SHALL keep the replies it holds, so that a reply added in the meantime, by an agent or another window, stays. A window SHALL keep the annotations and replies it added, changed or removed itself over a snapshot the server made before its operation, until a newer snapshot shows its change or two seconds have passed. A form in which the user is writing SHALL keep its text when another window or an agent changes the same thread.

#### Scenario: An older snapshot arrives
- **WHEN** a window adds an annotation and then receives a snapshot the server made before the operation arrived
- **THEN** the window keeps showing the annotation

#### Scenario: An agent replies while the user edits
- **WHEN** an agent adds a reply to annotation #1 while a window replaces #1's remark
- **THEN** #1 holds the new remark and the agent's reply

### Requirement: Anchoring By Quote
A window SHALL find each annotation's passage in the document it shows by the annotation's quote, in the text of the annotation's view: the document's source, or the text of its rendered view. Where the quote occurs more than once, the occurrence whose surrounding text matches the context best SHALL win, and among equal ones the one nearest the recorded lines. Where it does not occur, the window SHALL search again with runs of whitespace treated as one space. Where that fails as well, the window SHALL look for the passage the quote most likely became, near the recorded lines, and anchor the annotation there, marked as changed, when that passage is similar enough to the quote; otherwise the annotation SHALL be marked as not found in the current version. An annotation whose passage changed or was not found SHALL keep its recorded quote and lines, and SHALL be anchored without a mark again when a later version contains the quote. The window SHALL anchor again on every new version of the document and when the user switches between the views. The user SHALL be able to move an annotation to the passage currently selected in the pane, which becomes its target. The server SHALL store annotations without interpreting their text.

#### Scenario: Lines inserted above the passage
- **WHEN** an agent inserts ten lines above an annotated passage of a shown file
- **THEN** the passage stays marked at its new place, and the review names its new lines

#### Scenario: The passage reworded
- **WHEN** an agent rewrites one sentence of an annotated passage and keeps most of its words
- **THEN** the annotation is anchored at the reworded passage, marked as changed, and keeps its recorded quote

#### Scenario: The passage removed
- **WHEN** an agent rewrites the file without the annotated passage or anything similar near it
- **THEN** the annotation stays in the list, marked as not found in the current version, with its quote and recorded lines

#### Scenario: Moved to another passage
- **WHEN** the user selects a passage and moves an annotation whose passage was not found to it
- **THEN** the annotation's target is the selected passage, and it is no longer marked as not found

#### Scenario: The same words twice
- **WHEN** the quote of an annotation occurs twice in the document and only one occurrence has the recorded context
- **THEN** the annotation is anchored at that occurrence

### Requirement: Annotations In The Pane
A window SHALL mark the anchored passage of each open annotation in the view the annotation was taken from, and its lines in the other view. It SHALL list a pane's annotations inside the pane with their numbers and threads: the open ones, the anchored ones in document order, then those not found, then the general comments in creation order; and after them the resolved ones, folded. It SHALL show which entries were handed over to an agent, and show each reply of an agent as plain text with the title of the pane it came from. The user SHALL be able to hide the marks in the document, and to show the marks of resolved annotations, without changing the annotations; the window SHALL keep these choices per pane for its own lifetime. While a pane holds open annotations, its tab SHALL show a marker with their number.

#### Scenario: From the rendered view to the source
- **WHEN** the user annotates a passage in the rendered view of a Markdown file and switches to its source
- **THEN** the lines of that passage are marked in the source

#### Scenario: Marks hidden
- **WHEN** the user hides the marks of a pane
- **THEN** the document shows no marks, the list keeps the annotations, and other windows showing the pane keep their marks

### Requirement: Annotation Lifetime
A viewer pane's annotations, with their numbers, statuses and replies, SHALL live exactly as long as the pane: through moves, stacking, floating, docking and moves to another screen, a client's reload or reconnect, a reuse of the pane for the same document (see `viewer-panes`, Reopening A File), changes of the shown document, sending a review, and, in daemon mode, a daemon restart, update or crash. They SHALL end with the pane: when it or its screen is closed, or when an embedded server stops.

#### Scenario: Daemon restart
- **WHEN** a daemon restarts while a viewer pane holds annotations with replies
- **THEN** the pane holds the same annotations, numbers, statuses and replies afterwards

#### Scenario: Opened again from the CLI
- **WHEN** an agent runs `splitty open PLAN.md` again while the pane showing it holds annotations
- **THEN** the pane is reused and keeps its annotations

#### Scenario: Pane closed
- **WHEN** the user closes the pane and confirms
- **THEN** its annotations end with it

### Requirement: The Review
A window SHALL turn annotations of a pane into one Markdown text, the review. The review SHALL hold the pending annotations, the open ones with an entry of the user (the annotation itself or a reply) that the user has not handed over; when no annotation is pending, it SHALL hold all open annotations. It SHALL consist of:
- a heading with the file's name or the text document's title, and for a file its absolute path;
- each annotation of the review in the order of the pane's list, under its number, with its current lines, or its recorded lines and a note when its passage changed or was not found, and no lines for a passage of a rendered HTML page;
- for each, its kind, its quote as a block quote, its remark, and for a `change` its replacement in a fenced block that its text cannot close, or the request to delete the passage when the replacement is empty;
- for each, its replies in their order, each with its author;
- the general comments after the annotations with a passage;
- a last line naming the command with which an agent replies in a thread of this document (see `cli`, Comments Command).

The review SHALL contain nothing else. Control characters other than tab and line feed, and characters that change the direction of text, SHALL appear in the review as U+FFFD, wherever they come from, so that a pasted review cannot end a bracketed paste early or act as terminal input.

#### Scenario: A plan with a question and a suggested change
- **WHEN** a pane showing `/home/me/app/PLAN.md` holds a new comment #1 that asks a question on lines 12 to 14 and a new suggested change #2 on line 30
- **THEN** the review starts with a heading naming `PLAN.md` and the path, followed by #1 with lines 12-14, its quote and remark, #2 with line 30, its quote and its replacement, and a last line naming `splitty comments reply`

#### Scenario: A thread with an answer
- **WHEN** annotation #2 was handed over, an agent replied to it, and the user then replied to the agent
- **THEN** the next review holds #2 with its quote and remark, the agent's reply and the user's reply, each with its author, and no annotation without a new entry of the user

#### Scenario: A terminal selection
- **WHEN** a pane showing a terminal selection titled `Selection from claude` holds a comment
- **THEN** the review's heading names that title, no path follows, and the last line names the viewer pane

#### Scenario: A quote with an escape sequence
- **WHEN** an annotated passage of a document holds the characters ESC `[201~` followed by a line of text
- **THEN** the review shows U+FFFD in place of ESC, and pasting it ends no bracketed paste

### Requirement: Handing A Review To A Terminal
A window SHALL offer to send a pane's review to a terminal pane of the same workspace: by default the viewer's origin while it exists as a terminal pane, otherwise one the user chooses among the terminal panes of the screen. Sending SHALL paste the review into the terminal as the user's own paste would, without submitting it, and focus that terminal. When the terminal's program has not enabled bracketed paste and the review has more than one line, the window SHALL instead store the review as a file and insert its path, and say so. The window SHALL also offer to store the review as a file on that terminal's server with `upload` and insert its quoted path, as a drop with path insertion does, and, when that server announced `scratchpad-v1` and the result stays within the scratchpad's limit, to append it to the terminal's scratchpad. Sending in any of these forms SHALL mark the annotations and the user's replies the review held as handed over with `mark_sent`, and SHALL keep them; an entry added or changed in the meantime SHALL stay pending. Copying the review to the clipboard and saving it as a download of the window SHALL mark nothing.

#### Scenario: Send to the agent
- **WHEN** the user sends the review of a plan whose viewer was opened from the agent's terminal, and the agent's program has enabled bracketed paste
- **THEN** the review appears at the agent's prompt as a paste, nothing is submitted, the terminal has the focus, and the pane keeps its annotations, marked as handed over

#### Scenario: Undo after sending
- **WHEN** the user has sent a review
- **THEN** the window offers no undo, since the pane keeps the annotations it sent

#### Scenario: Sent again
- **WHEN** the user sends the review of a pane whose open annotations have all been handed over
- **THEN** the review holds all open annotations

#### Scenario: The origin is gone
- **WHEN** the viewer's origin pane has been closed and the user sends the review
- **THEN** the window asks for a terminal pane of the screen and sends nothing before the user chooses one

#### Scenario: A program without bracketed paste
- **WHEN** the user sends a review of several lines to a terminal whose program has not enabled bracketed paste
- **THEN** the review is stored as a file on that terminal's server, its path is inserted at the prompt, and a notice says so

#### Scenario: Into the scratchpad
- **WHEN** the user sends the review to the agent's scratchpad
- **THEN** the review is appended to the scratchpad's text, its area opens, and the terminal receives nothing

#### Scenario: Copied
- **WHEN** the user copies the review as Markdown
- **THEN** the clipboard holds the review, and no annotation is marked as handed over

#### Scenario: Added while sending
- **WHEN** window B adds an annotation to the pane while window A sends the review without it
- **THEN** B's annotation stays pending

## ADDED Requirements

### Requirement: Threads
Every annotation SHALL carry a thread of replies in the order they were added, at most 50. A reply SHALL have an ID of the form `rep-` followed by 16 lowercase hex characters, unique within its pane, an author, `user` or `agent`, and a text that is not only whitespace. The user's replies SHALL come from a window (see Saving Annotations Through The Window), with IDs the window chooses. An agent's reply SHALL come from a process in a pane through the control API (see Agents Read And Reply To Handed-Over Threads), with an ID the server chooses, and SHALL carry the pane it came from and that pane's title at that time, at most 256 bytes. The user SHALL be able to edit and remove the user's own replies and to remove an agent's replies; an agent's reply SHALL NOT be changed. Removing an annotation SHALL remove its replies.

#### Scenario: The user answers an agent
- **WHEN** annotation #2 holds an agent's reply and the user replies in the window
- **THEN** the thread holds the agent's reply followed by the user's reply with author `user`

#### Scenario: An agent's reply is not edited
- **WHEN** a window puts a reply with the ID of an agent's reply
- **THEN** the server rejects the operation, and the reply stays as the agent wrote it

### Requirement: Resolving A Thread
The user SHALL be able to resolve an open annotation and to reopen a resolved one; only a window SHALL change the status, as part of the annotation it puts. Resolving SHALL keep the annotation, its number and its replies. A resolved annotation SHALL be in no review, and its passage SHALL be marked only while the user has chosen to show resolved annotations.

#### Scenario: Resolved after the agent's answer
- **WHEN** the user resolves annotation #2 after an agent replied to it
- **THEN** #2 leaves the open annotations, appears folded among the resolved ones with its replies, and its passage is no longer marked

#### Scenario: Reopened
- **WHEN** the user reopens a resolved annotation
- **THEN** it is open again with its number and replies, and its passage is marked again

### Requirement: Agents Read And Reply To Handed-Over Threads
Annotations SHALL be made, changed, resolved and removed only through a window's `mux-protocol` connection. A process in a pane SHALL be able, through the control API (see `control-api`, Viewer Comment Requests), to read the annotations that the user has handed over on the viewer panes of the screen of the pane it names, each with the agents' replies and the replies of the user that the user has handed over, and to add a reply as an agent to such an annotation, open or resolved; nothing else. Annotations and replies of the user that the user has not handed over SHALL NOT be returned, so that an agent sees only what the user sent. `GET /api/mux/state` and `GET /api/mux/screen/{id}` SHALL leave annotations out, and `POST /api/mux/op` SHALL refuse the operations of Saving Annotations Through The Window (see `control-api`). A window SHALL never send an agent's reply anywhere unless the user sends a review that holds it.

#### Scenario: An agent reads the workspace
- **WHEN** a viewer pane holds annotations and a process in a pane requests `GET /api/mux/state` over the control socket
- **THEN** the viewer pane appears without annotations

#### Scenario: An agent tries to annotate
- **WHEN** a process in a pane posts `put_annotations` over the control socket
- **THEN** the server responds with status 400, and the pane's annotations stay unchanged

#### Scenario: An agent reads a sent review
- **WHEN** the user has sent a review holding annotations #1 and #2, holds a new annotation #3 that was not sent, and an agent in a pane of the same screen lists the comments
- **THEN** it receives #1 and #2 with their replies, and not #3

#### Scenario: An agent replies
- **WHEN** an agent in pane `p1`, titled `claude`, replies to annotation #2 with `Switched to on-demand computation.`
- **THEN** #2 holds a reply of author `agent` with that text, pane `p1` and title `claude`, and the windows showing the pane show it

#### Scenario: A reply to an annotation not sent
- **WHEN** an agent replies to an annotation the user has not handed over
- **THEN** the request fails as not found, and the annotation stays unchanged

#### Scenario: Another screen
- **WHEN** an agent in a pane of screen `s2` lists the comments
- **THEN** it receives none of the annotations on screen `s1`

### Requirement: Waiting For A Review
While a process waits for the user's verdict on a viewer pane's document (see `cli`, Review Command), the viewer record SHALL name the wait, the waiting pane and that pane's title, and a window showing the pane SHALL say so and offer to approve the document or to send the review to the waiting process. The window SHALL answer with `viewer.review_answer` (see `mux-protocol`, Review Answers). Approving SHALL answer without a review and mark nothing. Sending SHALL answer with the review, as Handing A Review To A Terminal builds it, and, once the server has accepted the answer, mark the review's entries as handed over. The server SHALL settle the wait with the first answer and refuse later ones. When the waiting process ends its request or the pane closes, the server SHALL end the wait, and the windows SHALL stop offering it.

#### Scenario: Feedback on a plan
- **WHEN** an agent runs `splitty review --text --title Plan` with its plan, and the user comments on two passages and sends the review
- **THEN** the command prints the review with both comments and exits 0, and the comments stay in the pane, marked as handed over

#### Scenario: Approved
- **WHEN** the user approves the document while an agent waits for it
- **THEN** the command prints nothing and exits 0

#### Scenario: The agent stops waiting
- **WHEN** the waiting command is interrupted
- **THEN** the windows no longer offer to answer, and the annotations stay as they are

#### Scenario: Two windows answer
- **WHEN** two windows answer the same wait
- **THEN** only the first answer reaches the command, and the other window marks nothing

## REMOVED Requirements

### Requirement: The Window Is The Only Channel For Annotations
**Reason**: Agents take part in a review: they read what the user handed over and reply in its threads. The window stays the only channel through which annotations are made, changed, resolved and removed.

**Migration**: Replaced by Agents Read And Reply To Handed-Over Threads, which keeps the control API's state queries without annotations and its refusal of the annotation operations.
