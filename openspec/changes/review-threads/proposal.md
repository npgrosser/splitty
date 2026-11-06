# Proposal

## Why

Annotations on a viewer pane end as soon as their review is sent, so a review leaves no record, a question cannot be followed up, and an agent's answer stays in the terminal, away from the passage it is about. Four kinds of annotation ask the user to classify a remark that reads for itself. An agent cannot take part in the review, and a harness that wants the user's verdict on a plan before it goes on has no way to wait for it. Builds on `splitty-open` and `review-pane` (roadmap 105 and 111).

## What Changes

- **Threads**: every annotation opens a thread. The user replies in the window, an agent with `splitty comments reply`; agent replies carry the pane they came from. Threads have a number per pane, which the review, the list and the CLI share, and a status, open or resolved; resolved threads fold away and leave the document unmarked.
- **Two kinds**: `comment` and `change`; a change with an empty replacement suggests deleting the passage. `question` and `delete` go.
- **Sending keeps everything**: sending a review marks what it held as handed over instead of removing it, and the next review holds only what is new (or everything open again when nothing is). The ten-second undo goes.
- **Hiding**: the marks in the document can be hidden, and resolved threads shown, per pane.
- **Edited passages**: a passage that is not found word for word is looked for by similarity near its recorded lines and marked as changed; a thread can be moved to the current selection.
- **Agents in the review**: through the control API, a process in a pane reads what the user handed over on the viewer panes of its screen and replies in those threads (`splitty comments`, `splitty comments reply`). It never sees what the user has not sent, and cannot create, change, resolve or remove anything. This replaces the rule that annotations do not cross the control API at all (`viewer-annotations`, The Window Is The Only Channel For Annotations).
- **Waiting for a review**: `splitty review <file>` and `splitty review --text` show a document and block until the user approves it or sends a review, which the command prints, so an agent harness can ask for a verdict on a plan. `--text` replaces the text of the review pane it opened before, so the threads of a plan carry over to its next revision.
- **Review format**: thread numbers, replies with their authors, deletions, and a last line naming the reply command.

Out of scope: comments bound to a file rather than to its viewer pane, editing a document, an MCP server (roadmap 104), timestamps on annotations and replies, notifications outside the window, agent-created threads, and anything for grant sockets.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `viewer-annotations`: threads with replies, numbers and statuses; two kinds; sending marks instead of removing; hiding; similarity anchoring and moving a thread; agents reading handed-over threads and replying; waiting for a review; the review's format.
- `viewer-panes`: `open_viewer` can replace the text of a text document the same pane opened under the same title.
- `cli`: `splitty comments`, `splitty comments reply` and `splitty review`.
- `control-api`: the comment and review-wait endpoints; the new window operations are refused.
- `mux-protocol`: nudges and rejections for the new operations, agent replies and waits; `viewer.review_answer` and its acknowledgement.
- `workspace-model`: the new window operations and the trusted viewer operations the server derives from control API requests.
- `access-control`: the new endpoints on the owner control socket, none on grant sockets.
- `server-modes`: waits for a review are not persisted.

## Impact

- `splitty-core` (`mux`): the annotation record, new operations, trusted operations for agent replies and waits, validation and persistence.
- `splitty-server`: the mux handler (`viewer.review_answer`), the control API endpoints and their routing, the CLI commands and the agent guide.
- `frontend/`: the store, the toolbar, the form, the thread list, anchoring, the review, the hand-over and the waiting banner.
- Docs: `docs/protocol.ts`, `docs/architecture.md`, `docs/files-and-editors.md`, the agent guide, README.
