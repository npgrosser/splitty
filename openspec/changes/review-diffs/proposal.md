# Proposal

> Draft created on 2026-11-06. Not planned yet. Specs and tasks are written when this change is next. Depends on: `splitty-open` (viewer panes), `review-pane` (annotations and the hand-over of a review). Carries over the diff parts of the `review-pane` draft of 2026-11-04.

## Why

After an agent's task, what needs review is the change itself, spread over several files. Today it is read with `git diff` in a terminal or in an external editor, and remarks on lines go back to the agent by hand. A viewer pane that shows the diff of a pane's repository, with the annotations and the review hand-over of viewer panes, closes that loop.

## What Changes

- **Diff documents**: a viewer pane shows the changes of the Git repository of a pane's directory: the working tree against the index, the index against `HEAD`, or the working tree against a commit; untracked files listed. A file list, and per file a side-by-side or unified diff.
- **Annotations on a side**: an annotation on a diff names the file, the side (old or new) and its lines, anchored by quote as in `review-pane`; the review lists them per file (`path:line, new side`).
- **One round across files**: a review of a diff covers all of its files, which may need a round at screen level instead of per pane.
- **Entry points**: a "Review Changes" entry in the terminal's context menu, and a CLI command for agents and users (`splitty diff` or `splitty review`, the name decided with the change), placed next to the calling pane as `splitty open` does.

## Capabilities

To be decided when the change is next. Likely candidates:

### Modified Capabilities
- `viewer-panes`: the `diff` source and its presentation.
- `viewer-annotations`: annotations on a diff side and the review across files.
- `cli`: the diff command.
- `access-control`: running `git` on the server on behalf of a window.

## Impact

- Server: read-only `git` calls in the repository with fixed arguments, paths after `--`, and repository settings that could run commands turned off (`--no-ext-diff`, `--no-textconv`, `core.fsmonitor` off, no pager); size limits per file and per diff; reading through the window's connection like `viewer.read`.
- Frontend: a diff view (CodeMirror's merge view), the file list, annotations per side.
- Open questions: the default comparison, and whether to offer "everything since this pane opened" through a baseline taken then; binary and very large diffs; renames; whether a round spans the files of one diff pane or several panes.
