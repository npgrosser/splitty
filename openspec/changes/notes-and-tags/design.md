# Design

> Draft design carried over from roadmap/103-notes-and-tags.md on 2026-10-09; revisit against the specs before implementation.

## Context

See proposal.md for motivation and scope. Constraints from the current system:

- The server owns one workspace of screens, panes and sessions (`workspace-model`). Every pane created by a layout operation is a terminal pane backed by a session; the model already knows the roles `webview` and `viewer`, but no operation creates them, and the frontend registers only the `terminal` component (the webview pane was removed in security review SR-01/SR-02).
- One WebSocket carries `mux.*` workspace messages and the session protocol (`mux-protocol`). Everything that does not start with `mux.` goes to the session protocol.
- Screens carry an ID, an optional name and the layout; `mux.welcome` lists only ID and name per screen.
- A daemon persists the workspace to `state.json` after every applied change; an embedded server keeps it in memory only (`server-modes`).
- Templates are YAML files under the config directory (`templates/<slug>.yaml`, written atomically through the owner-only directory helper) and are served over HTTP under `/api/templates`. They have no baseline spec.

## Goals / Non-Goals

**Goals:**
- Notes survive server restarts in both server modes, because they are not workspace state.
- Tags couple notes, screens and templates loosely (many-to-many), so deleting a screen never deletes a note.

**Non-Goals:**
- No CWD-based or per-screen scoping of notes (see Decisions).
- No explicit workspace entity.

## Decisions

### Tags instead of direct scoping

| Approach | Problem |
|---|---|
| CWD scoping | Splitty is started from an app launcher, not a terminal; there is no stable CWD. |
| Screen scoping (1:1) | Screens are ephemeral and their names unstable; the note would die with the screen. |
| Workspace entity | A large feature of its own; over-engineering for a start. |
| **Tags (M:N)** | Loose coupling; a note can belong to several contexts; deleting a screen kills no note. |

Tags are the workspace concept without an entity of their own: `tag:splitty` is everything that belongs to Splitty (screens, notes, templates). Explicit workspaces, if ever wanted, build on tags.

### Data model

```typescript
interface Note {
  id: string        // UUID
  content: string   // plain text
  tags: string[]
  createdAt: number // Unix timestamp
  updatedAt: number
}

// added to existing types
interface ScreenState { /* existing fields */ tags: string[] }
interface Template    { /* existing fields */ tags: string[] }
```

### Notes live outside the workspace store

Notes are not layout state, so they are not kept in the workspace store and do not go through the layout operations. A separate note store holds them, one JSON file per note (`<config dir>/notes/<uuid>.json`), created through the same owner-only directory rule as every other writer of the config directory (`access-control`). The roadmap doc's `~/.splitty/notes/` is replaced by the platform config directory Splitty already uses.

Tags on screens are part of the screen's state and change through the normal operation path. The roadmap doc sketched this as `{ type: "mux.op", op: { type: "update_screen", screenId, tags } }`; no `update_screen` op exists today (`rename_screen` is the closest), so the operation's name and whether it generalizes `rename_screen` is decided when the specs are written.

### Note messages on the WebSocket

```typescript
// client → server
{ type: "note.create", note: { content: string, tags: string[] } }
{ type: "note.update", noteId: string, content?: string, tags?: string[] }
{ type: "note.delete", noteId: string }
{ type: "note.list",   filterTags?: string[] }

// server → client
{ type: "note.snapshot", notes: Note[] }
{ type: "note.changed",  note: Note }
{ type: "note.deleted",  noteId: string }
```

These need a third route next to `mux.*` and the session protocol (see proposal, `mux-protocol`). Alternative to weigh when the change is next: templates and settings already use HTTP CRUD endpoints; notes could follow that precedent and use the WebSocket only for the `note.changed`/`note.deleted` broadcast.

### Note pane

A new pane kind next to `terminal`: a plain textarea or contenteditable, no xterm.js, no PTY. It integrates with dockview like a terminal pane (split, tab bar, drag) and auto-saves debounced. In the workspace model it is a pane that references a note ID instead of a session.

### UX

- **Create**: `Ctrl+Shift+N` opens a note pane; the note inherits the active screen's tags, editable. Minimal editor, plain text, no Markdown rendering.
- **Show**: a note list filterable by tags. On a screen tagged `[splitty]` it shows notes tagged `splitty` plus all untagged notes. Fuzzy search over title or first line.
- **Lifecycle**: persistent by default; explicit delete at any time; optional auto-cleanup marks untagged notes older than X days as stale.

## Architecture impact

- **Two Protocol Paths** becomes three: a `note.*` message family next to the session protocol and the mux protocol, with its own routing in the WebSocket handler (or HTTP endpoints plus a broadcast, if that alternative wins).
- **Module Map**: a note store module in the core next to `mux/` and `daemon/`; a note pane component and note list in the frontend.
- **Ops Model / domain types**: screens gain `tags`; panes gain a `note` kind that references a note instead of a session.
- **Server Modes**: notes are persisted in both modes, unlike the workspace, which only a daemon persists.

## Risks / Trade-offs

- [The workspace model assumes every pane has a session; spawn effects, respawn on daemon start and the client's `mux.pane_spawned` handling all build on it] → treat `note` as a pane kind with an explicit "no session" branch in each of these paths, specced as a MODIFIED of `workspace-model`.
- [Old layouts or templates naming an unknown pane kind make dockview throw] → the same stripping that already guards removed `webview` panes must recognize `note` panes on clients that do not support them.
- [Notes contain arbitrary private text in the config directory] → same owner-only directory rule as credential files; file modes decided with the specs.

## Open Questions

- Overlay or sidebar in addition to the pane kind?
- Tag autocomplete from existing tags, or free input?
- Should `splitty note "quick thought" --tags splitty` ship from the start? If so it needs a control endpoint and, for use inside panes, an extension of the control socket route set (`access-control`).
- Does `mux.welcome` list each screen's tags, or do clients learn them from snapshots only?
- Config directory or data directory for notes? Templates live in the config directory today.
