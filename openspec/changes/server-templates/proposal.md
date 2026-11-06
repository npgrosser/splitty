# Proposal

> Planned on 2026-11-06 from the draft of 2026-11-04. Part 1 of the sequence in `frontend-client-core` (design, "Sequence"); independent of parts 2 to 4. Related: `workspace-restore` (it rebuilds layouts with launch data too and may share the conversion), `review-pane` and `splitty-open` (pane kinds a template may hold later).

## Why

Templates are applied and extracted in the window. The server only stores them.

- **An applied template is invisible to the server.** The window builds the template's panes in dockview. Each pane creates its own session (`directSpawn`), and no mux op adds the pane to the screen, while the old panels each send `close_pane`. After the apply the server's screen is empty: other windows, the CLI and a reload do not see the template's layout or its sessions as panes (also noted in `docs/layout-sync-drift.md`).
- **"Apply in new screen" is fragile.** It sends `create_screen` with a throwaway pane, closes that pane, and assumes the new view mounts within one animation frame. When it does not, the template is silently not applied.
- **Extraction reads the DOM.** It takes the grid from dockview's `toJSON()` and floating bounds from DOM rectangles. The server already holds the root layout, the floating entries, each session's command, restore mode and shell profile, and the foreground process and working directory.
- **The window repeats server rules.** It resolves a template's relative directories, checks them twice (`TemplateDialog.svelte`, `template_apply.ts`), and checks shell profiles. The dispatcher already resolves a root plus a relative directory.

## What Changes

- **Apply in the window at once, authoritatively on the server.** The window keeps acting first (`frontend-client-core` CC-10).
  - It renders the template's layout immediately, with pane IDs it generates, through the same path that builds a screen from a snapshot.
  - It sends one mux op that carries the template's tree and those IDs, to replace a screen's layout or to create a new screen.
  - The reducer applies the same tree. The conversion is pure, as reducer invariants require, and the same on both sides.
  - The effect runner spawns each pane's session with the template's command, working directory (root plus relative directory), shell profile and restore mode.
  - The window attaches through `mux.pane_spawned`, as after a split. A rejected op comes back as `mux.op_failed`, and the window resyncs. Other windows get a nudge. Every window, the CLI and a reload see the result.
- **Extract on the server.** The server builds a template from one of its screens: layout, floating entries, and per pane its command, working directory, profile and restore mode. The window only names and saves the result.
- **Consent stays in the window.** A template's commands are still listed and confirmed before they run (SR-12). Only authenticated window session connections may apply templates; owner control and generic mux APIs reject this op.
- **The window stops building layouts.** The template dialog sends the op. `layout/template_apply.ts`, `layout/template_extract.ts` and "apply in new screen" with its throwaway pane are removed. Against a server without the new capability, applying is unavailable and the host's update is offered (`frontend-client-core` CC-9).

## Capabilities

The following contracts cover the authoritative template path:

### New Capabilities

- `templates`: what a template holds, storing, applying to a screen or a new screen, extracting from a screen, command consent.

### Modified Capabilities

- `workspace-model`: an op that replaces a screen's layout from a template, or creates a screen from one; whether the IDs of the new panes come from the client or the server.
- `mux-protocol`: the op, its capability flag, the resulting `mux.pane_spawned` and nudges.
- `access-control`: which connections may apply a template with commands.
- `control-api`: extraction (an HTTP read beside `/api/templates`), with no CLI template application.

## Impact

- **Core:** `template` (conversion to a layout and launch data), `mux` (op, validation, reducer), `effect_runner` (several spawns from one op).
- **Server:** `service` operations for templates, `client_session`'s mux handler, the template routes.
- **Frontend:** the template dialog and the template code in `Workspace.svelte`; two layout modules removed.
- **Docs:** `docs/protocol.ts`; the architecture model if `mux` gains a relationship to `template`.
- **Both transports:** new requests are added to the native operation catalog (`docs/native-operations.json`) as well as the HTTP routes, and new messages travel over the WebSocket and native connections alike (`native-client-transports`).

Out of scope: new template features (variables, pane kinds without a session), the template store's format, and sharing templates between servers.

## Carried-over design notes

- 2026-11-04, `frontend-client-core` inventory:
  - `template_apply.ts` creates every pane with `directSpawn: true` (lines 124, 135, 277, 284) and keeps module-level warning sets and toasts.
  - `template_extract.ts` reads floating bounds from DOM rectangles (about lines 135–148).
  - `Workspace.svelte` (about lines 629–645) holds "apply in new screen".
  - `create_job` shows that an op can carry explicit launch data.
