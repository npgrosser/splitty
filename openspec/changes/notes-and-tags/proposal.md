# Proposal

> Draft migrated from roadmap/103-notes-and-tags.md on 2026-10-09. Not planned yet. Specs and tasks are written when this change is next. Depends on: the baseline capabilities `workspace-model`, `mux-protocol` and `server-modes` (roadmap Milestone 1). Related: `splitty-open`, which also needs a pane that is not a terminal; `session-scratchpad`, which comes first as a quick win and gives every terminal session one scratch text bound to its lifetime (the 1:1 binding this change rejects for lasting notes). Keeping a scratchpad as a note is a natural bridge once both exist.

## Why

Developers keep jotting ideas, snippets and coding prompts into an open editor buffer during the day: scratch paper that is never saved as a file. Splitty should offer that as a deliberate tool: capture without a context switch (hotkey, type, carry on), kept private outside any repository, surviving sessions without claiming to be an archive. Notes need a loose link to what they are about, and the same link can group screens and templates into an implicit workspace.

## What Changes

- **Notes**: plain-text notes with tags, created, edited, listed and deleted by clients, persisted on the server independently of the workspace and of the server mode, one file per note, private to the user.
- **Note pane**: a new pane kind next to `terminal` that shows one note as a plain text field with debounced auto-save. It has no session and no PTY, and it splits, tabs and drags like any other pane. `Ctrl+Shift+N` opens a new note pane.
- **Tags on screens**: screens carry a list of tags, changed through a workspace operation like other screen attributes and persisted with the workspace in daemon mode.
- **Tags on templates**: templates carry a list of tags in their stored definition.
- **Tag-based filtering**: a new note inherits the active screen's tags (editable). On a screen tagged `[splitty]`, the note list shows notes tagged `splitty` plus all untagged notes. The list can be filtered by tags and fuzzy-searched over each note's title or first line.
- **Note messages on the client connection**: create, update, delete and list notes; the server answers with a snapshot and broadcasts changed and deleted notes to connected clients.
- **Lifecycle**: notes persist by default and can be deleted at any time. Optionally, untagged notes older than X days are marked stale.

Out of scope: Markdown or rich-text rendering, sharing notes between users, an explicit workspace entity (tags are the implicit workspace concept, and explicit workspaces can build on them later), full-text search, and sync between devices.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- `notes`: note records, tags, persistence, the note messages and their broadcast, filtering and the stale marker.

### Modified Capabilities
- `workspace-model`: screens gain tags and an operation to set them; a pane of the new `note` kind references a note instead of a session, while "Workspace Structure" currently requires every pane created by a layout operation to reference exactly one session.
- `mux-protocol`: "Shared Connection for Control and Terminal I/O" routes every non-`mux.` message to the session protocol, where an unknown type is answered with `error`; the `note.*` messages need their own route.
- `server-modes`: "Workspace Persistence In Daemon Mode" gains screen tags; notes persist in both modes, which the embedded-mode lifetime requirement should state explicitly so it is not read as "an embedded server writes nothing".
- `cli` and `access-control`, only if `splitty note` is part of the change: a control endpoint for notes is not in today's control socket route set ("Control Socket Route Set" lists the allowed routes exhaustively), so exposing it to processes in panes is a deliberate MODIFIED of that invariant.

## Impact

- Server: a note store outside the workspace store, the `note.*` message handling, screen tags in the workspace state and its persisted form, tags in the template definition.
- Frontend: a note pane component (dockview currently registers only `terminal` and throws on unknown components), the note list with tag filter and fuzzy search, the hotkey, and the tag editor for screens.
- Storage: the roadmap doc named `~/.splitty/notes/`. Splitty keeps its files in the platform config directory (`~/.config/splitty` on Linux), which `access-control` requires to be owner-only (mode 0700) for every writer, so notes follow that rule.
- `docs/protocol.ts` and `docs/architecture.md` (Two Protocol Paths, Module Map) need updating.
- Templates have no baseline spec yet; template tags either get specced inside `notes` or this change first adds a `templates` capability.

Open questions for the design:
- Should notes also be shown as an overlay or sidebar, or is the pane kind enough?
- Tag input: free text, or autocomplete from existing tags?
- Should the CLI path (`splitty note "quick thought" --tags splitty`) be part of the first version? It decides whether `cli`, `control-api` and `access-control` are touched at all.
