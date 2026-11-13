# Proposal

## Why

Two saves rest on rules each client makes up for itself. A saved template's file name is derived by each client from the template's name, and differently: the web client turns any run of other characters into `-`, the native client drops them and caps the name at 128 characters, so `v1.2` is saved as `v1-2` by one and as `v12` by the other, and each client's check for an existing file misses the other's. The check reads the library first and saves after, so it is not atomic either. A scratchpad save carries no version, so a window cannot tell a snapshot made before its save from one made after it; both clients hold their saved text against snapshots for a guessed two seconds instead. And the server types a whitespace-only template command into the terminal, while both clients treat such a command as none.

## What Changes

- New operation `POST /api/templates` (native `template.create`) with `{"template", "overwrite"}`: the server derives the file name from the template's name, saves only if no file of that name exists unless `overwrite` is true, and answers which file it wrote, or `409` with the existing file's name, atomically. Announced as `template-names-v1` in `mux.welcome`. Both clients send the name and drop their own file-name code; the native window's file name field goes away.
- Session records carry `scratchpadVersion`, raised by every accepted `set_scratchpad`; `set_scratchpad` takes an optional `ifVersion` and is refused when the scratchpad has another version. Announced as `scratchpad-versions-v1`. Both clients send the version their text is based on and ignore snapshots older than their own accepted save instead of the two-second window.
- A blank or whitespace-only command is no command on the server: template application records and launches no command for it, and a session start types and runs nothing for it.

Out of scope: listing on the server which panes of a template carry commands. The web client's template projection (`template_intent.ts`) gives a new or replaced screen its layout before the server's snapshot arrives; removing it means reworking how the web window starts a screen, which another change owns. It stays, with its own validation, and the command list stays a filter over the template in each client. Template application, extraction and the existing `POST /api/templates/{slug}` stay as they are.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `templates`: saved templates' file names come from the server; saving under a taken name needs an explicit overwrite; blank commands launch nothing; the native library has no file name field.
- `session-scratchpad`: versioned saves and how a window resolves snapshots against its own saves, with the older-server behavior.
- `terminal-sessions`: a whitespace-only startup command is no command.

## Impact

`crates/splitty-core` (template store and file names, scratchpad version and compare-and-set, template launch data, PTY startup), `crates/splitty-server` (new operation, welcome capabilities), `crates/splitty-client-core` (template save request, scratchpad op, capability flags), `crates/splitty-gpui` (template dialog, scratchpad editor), `frontend/` (template dialog, scratchpads), `docs/protocol.ts`, `docs/native-operations.json`, `docs/architecture.md`, `docs/native-ui-handoff.md`.

## Architecture impact

none (new behavior inside the existing template store, mux, server operations and client modules; no new crate, top-level module or dependency between components).

## Decisions

- SSR-1: a file name keeps the ASCII letters and digits of the trimmed name, lowercased; every run of other characters becomes one `-`, leading and trailing `-` are dropped, and the result is cut to 128 bytes. A name without an ASCII letter or digit is refused (provisional, tbd).
- SSR-2: the save by name is a new operation next to `POST /api/templates/{slug}`, which stays for older clients and scripts; it answers `200` with `slug` and `replaced`, and `409` with `slug` when the file exists and `overwrite` is not set. A save if absent creates the file only when none exists, from a complete temporary file, so of two windows saving the same new name one gets `409` (provisional, tbd).
- SSR-3: the native window learns the capability from its local service's `mux.welcome`; without it, it refuses to save and asks for the local service to be updated, and keeps no file-name rule of its own. The web window saves through the server that served it, which is the same build, so it needs no fallback (provisional, tbd).
- SSR-4: `scratchpadVersion` starts at 0 (absent on the wire) and rises by one with every accepted save, also an emptying one; the version is part of the session record and persists with it. `ifVersion` is optional, and a save without it replaces the text as before (provisional, tbd).
- SSR-5: a window sends the version its shown text is based on and then expects its save to be that version plus one. A snapshot with a lower version is ignored while no edit waits; one with that version or a higher one replaces the shown text. When another window's save reached the server first, the window's save is refused and the other text shows: in that race the first save wins. An edit that waits still wins over arriving snapshots, as before, by taking their version for its save. A new connection forgets the versions (provisional, tbd).
- SSR-6: without `scratchpad-versions-v1`, a window sends no `ifVersion` and keeps holding its saved text against snapshots for two seconds, as before (provisional, tbd).
