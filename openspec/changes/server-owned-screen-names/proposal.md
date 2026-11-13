# Proposal

## Why

New Screens are named "Screen N" in four places: the server (for a create without a name), the web store (`nextScreenName`, plus a rule that renames a pending host Screen whose default name the host already uses), the native window (explicit names on every local create) and positional fallback labels ("Screen {index+1}") that can repeat the name of a real Screen. The server is the only place that knows every name of its workspace, so it should name new Screens alone. The first Screen of an empty workspace is created by each client on its own; two windows that start together can create two.

## What Changes

- The server announces `screen-names-v1` in `mux.welcome`: it names every Screen created without a name (`create_screen`, `move_pane_to_screen` with `createScreen`) with the first unused "Screen N" of its workspace, counting from 1, and takes `onlyIfEmpty: true` on `create_screen`.
- `create_screen` with `onlyIfEmpty: true` is applied only to a workspace without Screens; otherwise it is refused with `mux.op_failed` and nothing changes. Both clients send their first Screen this way, so windows that start together end with exactly one; the window whose create was refused drops its own Screen and shows the server's.
- Both clients omit the name of a new Screen when the server announces the capability and show the server's name once it arrives; the web client resyncs its new Screen to learn it, and takes the names of local Screens from their snapshots. The web client no longer renames pending host Screens: the host picks the name when it creates the Screen.
- Against a server without the capability a client sends "Screen N", the smallest positive N no Screen of that server uses as far as the window knows, computed when the create goes out.
- No positional fallback label: a Screen the server reports without a name shows as "Unnamed Screen" in both clients, a Screen whose creation is pending as "New Screen" (web) or "Creating Screen…" (native).

Out of scope: rename and its validation, IDs, the placement of new Screens, the native window's empty page and "New Screen" action (unchanged apart from omitting the name).

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `workspace-model`: the first Screen of an empty workspace (`onlyIfEmpty`).
- `mux-protocol`: the `screen-names-v1` capability, server-chosen names in both clients, the first Screen with `onlyIfEmpty` in Client Resynchronization, older-server behavior.
- `remote-screens`: Screen Default Names Per Host come from the host, not from the window.

## Impact

`crates/splitty-core` (mux op and reducer, capability constant), `crates/splitty-server` (session and HTTP `create_screen`, welcome capability), `crates/splitty-client-core` (connection, catalog naming and first Screen, labels), `crates/splitty-gpui` (create without a name, fallback labels), `frontend/` (screens store, screen startup, remote screen sync, workspace), `docs/protocol.ts`, `docs/architecture.md`, `docs/native-ui-handoff.md`.

## Architecture impact

none (no new crate, top-level module or dependency between components).

## Decisions

- SSN-1: the first Screen is an idempotent create (`onlyIfEmpty: true`) that each client sends, not a Screen the server creates by itself on connect or start: a host reached through the hub, the CLI and tests connect to the same servers and must not get a Screen nobody asked for (provisional, tbd).
- SSN-2: against an older server the client names the Screen with the smallest free "Screen N" it knows of when the create goes out; a pending host Screen is no longer renamed when the host's list arrives later. The native window's host Screens keep going out without a name, as they did before (provisional, tbd).
- SSN-3: until the server's name arrives a window shows a placeholder label that it never sends ("New Screen" on the web, "Creating Screen…" natively); a Screen without a name shows as "Unnamed Screen" (provisional, tbd).
- SSN-4: an explicitly empty name (create or rename) stays empty on the server, as before; only the clients' label for it changes.
