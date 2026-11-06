# Proposal

> Completed on 2026-11-06 from the draft of 2026-11-04; the draft's open questions are decided in `design.md`. Roadmap: 111 (Review-Pane). Depends on: `splitty-open` (105), which introduces the viewer panes this change annotates. Related: `session-scratchpad` (a review can go into a scratchpad to be edited before it is sent), `notes-and-tags` (lasting notes, as opposed to the one-off review rounds here). The parts of the draft that this change does not cover moved to the drafts `review-diffs` (diffs of a pane's repository) and `viewer-quick-edit` (editing small files in a viewer pane).

## Why

With coding agents, most of the reading next to the terminal is review: a plan or a report the agent wrote, a file it changed, a long answer it printed. Feedback on specific passages goes back to the agent as hand-copied quotes today. Viewer panes now show the text next to the agent's terminal; what is missing is marking passages, writing a remark on each, and handing the collected remarks to the agent in one piece, without integrating with any agent harness.

## What Changes

- **Annotations on a viewer pane**: select a passage in a viewer, in the rendered view or the source, and add a comment, a question, a suggested change or a deletion; or add a general comment without a passage. The passages are highlighted, and a list in the pane shows all annotations in document order.
- **Anchored by quote**: an annotation keeps the quoted text, a little context before and after it, and its source lines. When the file changes, it is found again by its quote; when the quote is gone, it is marked as such and kept, never dropped silently.
- **Kept on the server, one review round per pane**: the annotations of a viewer pane are saved with it on the server, so they survive a reload, show in a second window and, in daemon mode, a restart. Closing a pane with annotations that were not handed over asks first.
- **A review as Markdown**: the annotations become one Markdown text with the document's path, each passage's lines and quote, and the remark.
- **Hand-over**: send the review to a terminal, by default the one the viewer was opened from, pasted at its prompt without Enter, or as a file whose path is inserted, or into the terminal's scratchpad to edit it first. Sending ends the round, with a moment to undo. Copying the Markdown and saving it as a file keep the round.
- **Agents never read pending annotations**: the control API neither shows nor changes them; an agent sees a review only when the user sends it.

Out of scope:
- Diffs of a repository with annotations (`review-diffs`), editing files (`viewer-quick-edit`).
- Lasting comments stored in the repository or inline in files, threads, replies or resolution by the agent.
- Hooks into a specific agent harness, such as a plan mode.

## Capabilities

### New Capabilities
- `viewer-annotations`: annotations and their limits, saving them through the window, anchoring by quote, their lifetime with the pane and the confirmation before they are lost, the review as Markdown, handing it to a terminal, copying and saving, the window as the only channel, and the capability `viewer-annotations-v1`.

### Modified Capabilities
- `workspace-model`: "Layout Operations" gains `put_annotations` and `remove_annotations`, which only `mux-protocol` accepts.
- `mux-protocol`: "Change Propagation to Other Clients" nudges the other clients after both operations.
- `control-api`: "Workspace and Screen Queries" omit annotations, and "Submitting Layout Operations" refuses both operations.
- `access-control`: "File Transfers Need The User" admits the upload of a review the user sends to a pane as a file.
- `server-modes`: "Workspace Persistence In Daemon Mode" keeps the annotations with their viewer pane.

## Impact

- `splitty-core`: `annotations` in the viewer record, the screen operations `put_annotations` and `remove_annotations` with their validation.
- `splitty-server`: the nudges after both operations, `viewer-annotations-v1`, the control API's answers and its refusal.
- Frontend: the annotation store per connection, the selection toolbar and the annotation form, highlights in the source view (CodeMirror decorations and gutter markers) and the rendered view, the annotation list, anchoring, the Markdown review, the hand-over actions, the close guard and the quit confirmation.
- Docs: `docs/protocol.ts`, `docs/architecture.md`, `docs/files-and-editors.md`, README.

## Prior art

Plannotator (plannotator.ai, MIT or Apache-2.0, runs locally) reviews agent plans through the plan step's hook and offers commands to annotate Markdown files, local diffs and an agent's last message; its feedback goes back to the agent as structured text. This change differs in that the user starts a review at any time while the agent keeps working, it covers any text document including code and terminal selections, the review sits in the layout next to the terminal and works the same on remote hosts, and it does not depend on the agent harness.
