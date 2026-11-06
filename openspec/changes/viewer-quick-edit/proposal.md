# Proposal

> Draft created on 2026-11-06. Not planned yet. Specs and tasks are written when this change is next. Depends on: `splitty-open` (viewer panes and the read path). Related: `review-pane` (annotations describe a change; quick edit makes it), the editor hand-over (`splitty open --editor`).

## Why

Next to a coding agent, the files a developer edits by hand are often small: an agent instruction file, a configuration, a prompt, a plan before the agent starts. A viewer pane already shows them on any host; editing them means switching to an external editor, which on a remote host needs a remote setup of its own.

## What Changes

- **An edit mode** in a viewer pane of a file: the source view becomes editable, with the viewer's highlighting and search, and the pane marks unsaved changes.
- **Saving on the server** through the window's connection, only on the user's save: only the file the pane shows, only an existing regular file, within the viewer's size limit. A save is refused when the file changed since it was loaded, since the agent may be editing the same file; the window then offers to reload or to compare.
- **Confirmation** before unsaved changes are lost, as for a scratchpad.
- Deliberately without language servers, extensions, project-wide search or new files; beyond that, the editor hand-over applies.

## Capabilities

To be decided when the change is next. Likely candidates:

### Modified Capabilities
- `viewer-panes`: the edit mode, saving with a version check, unsaved changes.
- `access-control`: writing a file on behalf of a window. No operation replaces files today (uploads never do), so this needs its own rules: only the user's save, only the pane's connection, never the control API or a grant socket, regular files only, how symbolic links and permissions are treated.

## Impact

- Server: a `viewer.write` message beside `viewer.read`, a write that keeps the file's permissions and fails rather than replacing a changed file.
- Frontend: CodeMirror's editable mode, the unsaved state, the conflict prompt.
- Open questions: writing in place or through a temporary file and a rename (keeps hard links and ownership versus atomicity); whether to keep a backup of the previous version; whether a `text` document (a terminal selection) can be edited and saved as a new file.
