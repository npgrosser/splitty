# Proposal

> Draft created on 2026-11-04. Not planned yet. Specs and tasks are written when this change is next. Depends on: `splitty-open` (a pane kind without a session, the renderers and the sandbox decisions; the two drafts may merge when one of them is next). Related: `notes-and-tags` (lasting notes, as opposed to the one-off review rounds here), `session-scratchpad` (where a review can be edited before it is sent), `mcp-server` (not needed for the agent entry point described here). Before this is next: use an existing annotation tool for agent work (see Prior art) for a while to learn which parts are actually missing.

## Why

With coding agents, the editor next to the terminal is used less for writing code and more for reading and reviewing it: what the agent changed, a plan or a document it wrote, a line to correct. Today that means leaving Splitty for a second window, and feedback on specific lines goes back to the agent as hand-copied quotes. A pane that shows files and diffs next to the agent's terminal, lets the user annotate lines and hands the collected annotations to the agent as text closes that loop without integrating with any agent harness.

## What Changes

- **Review pane**: a pane kind without a session that shows either a file (Markdown rendered; code and other text with syntax highlighting; images and PDF through the renderers of `splitty-open`) or a diff of a pane's repository (unstaged, staged or against a commit), side by side or unified. Read-only in the first stage.
- **Annotations**: on a line range of a shown file or diff, the user adds an annotation of a kind (comment, question, suggested replacement, deletion) with text. An annotation records the path, the line range, the quoted text and, for a diff, the side. The quote anchors it: after the file changes, the annotation is re-anchored by its quote and marked stale if the quote is gone, never dropped silently.
- **Review rounds, not lasting comments**: annotations collect as one pending review across several files and diffs, as in a pull-request review, kept on the server so they survive a reload, until the user sends or discards them.
- **Sending to a terminal**: sending writes the review as Markdown into a cache on the target terminal's machine (next to the paste cache) and inserts its path, or the text of a short review, at the terminal's prompt without pressing Enter; alternatively into the terminal's scratchpad for editing first. The target is the terminal the pane was opened from, or one the user picks. The agent reads the file with its own tools, so any agent works.
- **Entry points**: the terminal and tab context menus ("Review Changes" for the repository of the terminal's directory, "Open in Splitty" for a file) and the CLI (`splitty open --in splitty <file>`, the destination `splitty-open` reserves, and a command for diffs; names provisional). The same CLI is the agent's entry point through the scoped agent socket: after a task, an agent opens a diff or a file next to its own pane, and its pane is the default target of the review.
- **Quick edit (later stage)**: editing small text files in the same pane (an agent instruction file, a configuration, a prompt file) and saving them on the server. Deliberately without language servers, extensions or project-wide search; beyond that, the existing editor handoff (`splitty open --with`) applies.

Out of scope:
- Embedding a full VS Code or a VS Code extension host, and Splitty as a VS Code extension (see design.md, Considered alternatives).
- Lasting comments stored in the repository or inline in files; lasting notes belong to `notes-and-tags`.
- Replies from the agent per annotation, or the agent resolving annotations; the agent answers in its terminal.
- Hooks into a specific agent harness, such as a plan mode.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- `review-pane`: what the pane shows (file, diff), annotations and their anchoring, pending reviews, sending a review to a terminal.

### Modified Capabilities
- `workspace-model`: a pane kind without a session (shared with `splitty-open` and `notes-and-tags`), and what closing, moving, floating and persisting mean for it.
- `mux-protocol`: reading a file or a diff from the server for a client, the pending review state, the send operation.
- `cli`: the open and review commands and their availability to scoped agent callers.
- `access-control`: reading files and running `git` on the server on behalf of a client and which clients may do so (the local hub, isolated remote windows, the agent socket); for quick edit, replacing an existing file, which no operation does today (uploads never replace files).

## Impact

- **Frontend**: an editor component (Monaco or CodeMirror 6) in dockview, which today renders only terminal panes; the annotation UI; bundle size in the desktop apps.
- **Server**: file and diff reads for a client (paths on the server's machine, size limits), `git` in a pane's repository, a review cache with the paste cache's cleanup.
- **Security**: showing code as text executes nothing and needs no frame; rendered Markdown and HTML follow the sandbox requirements of `splitty-open` (SR-01, CSP `frame-src 'none'`, SR-27).
- **Remote**: content travels through the protocol, since the UI may run on another machine than the server.

## Prior art

Plannotator (plannotator.ai, MIT or Apache-2.0, runs locally) reviews agent plans through the plan step's hook and offers commands to annotate Markdown files and folders, to review local diffs and pull requests with line comments, and to annotate the agent's last message; the feedback goes back to the agent as structured text. It is started from the agent session and opens its own browser window. This change differs in that the user starts it at any time while the agent keeps working, it covers any file including code, it sits in the layout next to the terminal, it works the same on remote hosts, and it does not depend on the agent harness.
