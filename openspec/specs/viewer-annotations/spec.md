# viewer-annotations Specification

## Purpose

Annotations are a review of a document in a viewer pane: comments, questions, and suggested changes or deletions, each on a quoted passage or on the document as a whole. They are kept with the pane and find their passage again by its text when the file changes. The window turns them into a review in Markdown and hands it to the terminal the document was opened from, usually an agent's, or to another terminal, the scratchpad, the clipboard or a file. Annotations are made and read only through the window: processes in panes cannot read or change them and see a review only when the user sends it.

## Requirements

### Requirement: Annotations On A Viewer Pane
A viewer pane (see `viewer-panes`) SHALL carry a list of annotations, empty when the pane opens. Each annotation SHALL have an ID chosen by the window, of the form `ann-` followed by 16 lowercase hex characters, unique within its pane, and a kind: `comment`, `question`, `change` (a suggested replacement) or `delete` (a suggested deletion). An annotation SHALL either name a passage as its target, with the view it was taken from (`source` or `rendered`), the quoted text, up to 32 characters of context before and after it, and the source lines of the passage when it was annotated, or name no passage as a general comment, which SHALL be a `comment` or a `question`. A `comment` and a `question` SHALL carry a remark; a `change` SHALL carry a replacement that is not empty and MAY carry a remark; a `delete` MAY carry a remark. A pane SHALL hold at most 100 annotations and at most 262,144 bytes of text over all of them; a remark, a replacement and a quote SHALL be at most 16,384 bytes each and a context at most 256 bytes; lines SHALL lie between 1 and 10,000,000 with the first not after the last. The server SHALL refuse an operation that breaks these rules, or names a pane that is not a viewer pane, as a whole and without changing the workspace. A window SHALL offer annotations for documents shown as text, rendered or as source, and none for images and binary files.

#### Scenario: A question on a passage
- **WHEN** the user selects `retry three times` on line 30 of the source of `PLAN.md` and adds a question
- **THEN** the pane holds one annotation of kind `question` with that quote, view `source`, lines 30 to 30, its context and the remark

#### Scenario: A general comment
- **WHEN** the user adds a comment without selecting a passage
- **THEN** the pane holds an annotation of kind `comment` without a target

#### Scenario: One annotation too many
- **WHEN** a window puts a new annotation on a pane that holds 100 annotations
- **THEN** the server rejects the operation, and the pane keeps its 100 annotations

#### Scenario: A terminal pane
- **WHEN** a window puts an annotation on a terminal pane
- **THEN** the server rejects the operation, and the workspace stays unchanged

### Requirement: Saving Annotations Through The Window
A client SHALL add or replace annotations with the screen-level operation `put_annotations`, carrying the viewer pane's `paneId` and 1 to 100 annotations, and remove them with `remove_annotations`, carrying the pane's `paneId` and 1 to 100 annotation IDs, over `mux-protocol`. `put_annotations` SHALL replace an annotation with the same ID in its place and append a new one; `remove_annotations` SHALL ignore IDs the pane does not hold. The server SHALL keep the annotations in the viewer record, which screen snapshots carry as `annotations` in creation order, absent while empty, and SHALL let the other connected clients know (see `mux-protocol`, Change Propagation to Other Clients).

#### Scenario: A second window sees the annotation
- **WHEN** window A adds an annotation to a viewer pane while window B shows the same pane
- **THEN** window B shows the annotation after its resync, and A receives nothing for its operation

#### Scenario: Two windows add at the same time
- **WHEN** windows A and B each add an annotation to the same pane before either has seen the other's
- **THEN** the pane holds both annotations

### Requirement: Concurrent Annotation Edits
Edits of one annotation from several windows SHALL resolve as last write wins. A window SHALL keep the annotations it added, changed or removed itself over a snapshot the server made before its operation, until a newer snapshot shows its change or two seconds have passed. A form in which the user is writing SHALL keep its text when another window changes the same annotation.

#### Scenario: An older snapshot arrives
- **WHEN** a window adds an annotation and then receives a snapshot the server made before the operation arrived
- **THEN** the window keeps showing the annotation

### Requirement: Anchoring By Quote
A window SHALL find each annotation's passage in the document it shows by the annotation's quote, in the text of the annotation's view: the document's source, or the text of its rendered view. Where the quote occurs more than once, the occurrence whose surrounding text matches the context best SHALL win, and among equal ones the one nearest the recorded lines. Where it does not occur, the window SHALL search again with runs of whitespace treated as one space. An annotation whose quote is not found SHALL be marked as not found in the current version and kept with its recorded quote and lines; it SHALL be found again when a later version contains the quote. The window SHALL anchor again on every new version of the document and when the user switches between the views. The server SHALL store annotations without interpreting their text.

#### Scenario: Lines inserted above the passage
- **WHEN** an agent inserts ten lines above an annotated passage of a shown file
- **THEN** the passage stays marked at its new place, and the review names its new lines

#### Scenario: The passage removed
- **WHEN** an agent rewrites the file without the annotated passage
- **THEN** the annotation stays in the list, marked as not found in the current version, with its quote and recorded lines

#### Scenario: The same words twice
- **WHEN** the quote of an annotation occurs twice in the document and only one occurrence has the recorded context
- **THEN** the annotation is anchored at that occurrence

### Requirement: Annotations In The Pane
A window SHALL mark each anchored passage in the view its annotation was taken from, and its lines in the other view. It SHALL list a pane's annotations inside the pane: the anchored ones in document order, then those not found, then the general comments in creation order. While a pane holds annotations, its tab SHALL show a marker with their number.

#### Scenario: From the rendered view to the source
- **WHEN** the user annotates a passage in the rendered view of a Markdown file and switches to its source
- **THEN** the lines of that passage are marked in the source

### Requirement: Annotation Lifetime
A viewer pane's annotations SHALL live exactly as long as the pane: through moves, stacking, floating, docking and moves to another screen, a client's reload or reconnect, a reuse of the pane for the same file (see `viewer-panes`, Reopening A File), changes of the shown document and, in daemon mode, a daemon restart, update or crash. They SHALL end with the pane: when it or its screen is closed, or when an embedded server stops.

#### Scenario: Daemon restart
- **WHEN** a daemon restarts while a viewer pane holds annotations
- **THEN** the pane holds the same annotations afterwards

#### Scenario: Opened again from the CLI
- **WHEN** an agent runs `splitty open PLAN.md` again while the pane showing it holds annotations
- **THEN** the pane is reused and keeps its annotations

#### Scenario: Pane closed
- **WHEN** the user closes the pane and confirms
- **THEN** its annotations end with it

### Requirement: Confirmation Before Annotations Are Lost
Closing a viewer pane that holds annotations SHALL ask first, as closing a pane whose scratchpad holds text does; closing several panes at once SHALL ask once for all. Quitting a window whose embedded server would end viewer panes that hold annotations SHALL ask first as well. Quitting a window of a daemon SHALL NOT ask for annotations, since its panes continue.

#### Scenario: Close a viewer with annotations
- **WHEN** the user closes a viewer pane that holds three annotations
- **THEN** a confirmation names the annotations, and the pane closes only when the user confirms

#### Scenario: Quit an embedded window
- **WHEN** the user quits a window of an embedded server while a viewer pane holds annotations
- **THEN** the window asks before quitting

### Requirement: The Review
A window SHALL turn a pane's annotations into one Markdown text, the review: a heading with the file's name or the text document's title; for a file, its absolute path; then each annotation in the order of the pane's list, numbered, with its current lines, or its recorded lines and a note when its passage was not found, no lines for a passage of a rendered HTML page, its kind, its quote as a block quote, its remark, and for a `change` its replacement in a fenced block that its text cannot close; and the general comments last. The review SHALL contain nothing else. Control characters other than tab and line feed, and characters that change the direction of text, SHALL appear in the review as U+FFFD, wherever they come from, so that a pasted review cannot end a bracketed paste early or act as terminal input.

#### Scenario: A plan with a question and a suggested change
- **WHEN** a pane showing `/home/me/app/PLAN.md` holds a question on lines 12 to 14 and a suggested change on line 30
- **THEN** the review starts with a heading naming `PLAN.md` and the path, followed by the question with lines 12-14, its quote and remark, and the change with line 30, its quote and its replacement

#### Scenario: A terminal selection
- **WHEN** a pane showing a terminal selection titled `Selection from claude` holds a comment
- **THEN** the review's heading names that title, and no path follows

#### Scenario: A quote with an escape sequence
- **WHEN** an annotated passage of a document holds the characters ESC `[201~` followed by a line of text
- **THEN** the review shows U+FFFD in place of ESC, and pasting it ends no bracketed paste

### Requirement: Handing A Review To A Terminal
A window SHALL offer to send a pane's review to a terminal pane of the same workspace: by default the viewer's origin while it exists as a terminal pane, otherwise one the user chooses among the terminal panes of the screen. Sending SHALL paste the review into the terminal as the user's own paste would, without submitting it, and focus that terminal. When the terminal's program has not enabled bracketed paste and the review has more than one line, the window SHALL instead store the review as a file and insert its path, and say so. The window SHALL also offer to store the review as a file on that terminal's server with `upload` and insert its quoted path, as a drop with path insertion does, and, when that server announced `scratchpad-v1` and the result stays within the scratchpad's limit, to append it to the terminal's scratchpad. Sending in any of these forms SHALL end the round: the window SHALL remove the annotations it sent and, for ten seconds, offer to put them back; an annotation added in the meantime SHALL stay. Copying the review to the clipboard and saving it as a download of the window SHALL keep the annotations.

#### Scenario: Send to the agent
- **WHEN** the user sends the review of a plan whose viewer was opened from the agent's terminal, and the agent's program has enabled bracketed paste
- **THEN** the review appears at the agent's prompt as a paste, nothing is submitted, the terminal has the focus, and the pane holds no annotations

#### Scenario: Undo after sending
- **WHEN** the user chooses undo within ten seconds after sending
- **THEN** the pane holds the sent annotations again, and the pasted text stays at the prompt

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
- **THEN** the clipboard holds the review, and the pane keeps its annotations

#### Scenario: Added while sending
- **WHEN** window B adds an annotation to the pane while window A sends the review without it
- **THEN** the pane keeps B's annotation after A's round ends

### Requirement: Annotations Capability
The server SHALL announce `viewer-annotations-v1` in `mux.welcome.capabilities`. A client SHALL offer annotations on a server's viewer panes only when that server announced it, so that no annotation is written that a server would not keep.

#### Scenario: Host without annotations
- **WHEN** a window connects to a host whose daemon announces `viewer-v1` but not `viewer-annotations-v1`
- **THEN** the window shows that host's viewer panes and offers no annotations

### Requirement: The Window Is The Only Channel For Annotations
Annotations SHALL be shown and changed only through a window's `mux-protocol` connection. The control API, and with it the CLI and the processes in panes, SHALL neither return annotations nor change them (see `control-api`), so that an agent sees a review only when the user sends it to its terminal.

#### Scenario: An agent reads the workspace
- **WHEN** a viewer pane holds annotations and a process in a pane requests `GET /api/mux/state` over the control socket
- **THEN** the viewer pane appears without annotations

#### Scenario: An agent tries to annotate
- **WHEN** a process in a pane posts `put_annotations` over the control socket
- **THEN** the server responds with status 400, and the pane's annotations stay unchanged
