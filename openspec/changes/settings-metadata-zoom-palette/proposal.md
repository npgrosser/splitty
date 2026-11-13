# Proposal

## Why

Two appearance rules are kept by each client for itself. The zoom levels the keys step through (50 % to 300 % in thirteen steps) are a list in the web client and another in the native client, while the server's settings metadata publishes `appearance.zoom` with `step 0.1`, which neither list follows; the native settings editor offers the 0.1 grid, so it stores levels the keys skip, and the native window clamps every level to 0.5–3.0 for display and its steps, so a server that allowed more would store a level the window does not show. The web client rounds a zoomed terminal font size to whole pixels and the native client does not.

The terminal palette precedence (`terminal.theme`, then `terminal.themeDark`/`terminal.themeLight` by the mode of the shown UI theme, then the UI theme's own palette) exists only in the native client. The web client writes `terminal.theme`, but nothing reads it: its terminals always show the UI theme's colors.

## What Changes

- The server's settings metadata lists the zoom levels as the `values` of `appearance.zoom` (bounds 0.5 and 3.0, no `step`), and the server refuses a settings update with another level. Both clients step through the listed levels and offer them in their settings; the native window takes the bounds of its display from them instead of a fixed range.
- With the `terminals` scope a zoomed terminal font size is rounded to whole pixels in both clients.
- One palette rule in the server's core: `terminal.theme` unless `default`, else the palette for the shown UI theme's mode, else the UI theme's own palette. The server applies it in `theme.get` / `GET /api/home/theme`: the answer's `terminal` colors are those of the chosen palette where the server has a theme of that name, and the new `terminalPalette` names the theme they come from. The web client reloads the theme when a terminal palette setting changes. The native client applies the same rule, with the mode of its own catalog's UI theme, and keeps resolving names in its own catalog.

Out of scope: one shared palette catalog (theme files stay per client, so a palette name only one client has shows the UI theme's palette in the other), web controls for `terminal.themeDark`/`terminal.themeLight`, a notice in the web client for a palette the server lacks, and the native Drawer width clamp.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `display-zoom`: the levels come from the server's settings metadata; rounding of the zoomed terminal font size; the native window's bounds.
- `settings`: the metadata's zoom levels and their validation; the terminal palette precedence for both clients and the `theme.get` answer.

## Impact

`crates/splitty-core` (settings metadata and validation, theme palette rule), `crates/splitty-server` (`theme.get` answer), `crates/splitty-client-core` (zoom levels, zoom executor, palette rule for the native client), `crates/splitty-gpui` (zoom levels from the snapshot, terminal font rounding, palette rule), `frontend/` (zoom levels from metadata, theme reload on palette settings), `docs/protocol.ts`, `docs/architecture.md`.

## Architecture impact

none (new behavior inside the existing settings, theme and zoom modules; no new crate, top-level module or dependency between components).

## Decisions

- SMZ-1: the server publishes the thirteen levels the clients used as the `values` of `appearance.zoom` and drops its `step`; a settings update with a level outside them is refused. A stored level between them, written by hand, still loads, and the keys step from it to its neighbours (provisional, tbd).
- SMZ-2: a native window whose local service publishes no zoom levels (an older service, with or without metadata) steps through the levels of its own build's server core, within that service's bounds; the web window is served by the same build and without metadata only resets to 100 % (provisional, tbd).
- SMZ-3: with the `terminals` scope the terminal font size is the configured or native default size times the level, rounded to whole pixels, in both clients; with the `window` scope the native window scales terminals unrounded, as page zoom does (provisional, tbd).
- SMZ-4: the palette rule lives in the server's core. The web client gets its result in the `theme.get` answer; the native client applies the same rule in-process, because the mode it depends on is that of the native catalog's UI theme, which the server does not know, and the native window does not request `theme.get` (provisional, tbd).
- SMZ-5: palette names are resolved per client: the server's themes for the web client, the native catalog (including Ghostty palettes) for the native client. A name only one of them has shows the UI theme's palette in the other, without a notice on the web (provisional, tbd).
