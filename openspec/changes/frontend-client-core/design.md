# Design

> Draft. Written with the proposal on 2026-11-04 from an inventory of `frontend/src` at that date. Line references are approximate and will drift; the decisions are the point.

## Context

The frontend has 23,000 non-test lines of TypeScript and Svelte. Classified by what each module needs at runtime:

| Kind | Lines | Examples |
|---|---|---|
| UI-free logic | ~7,600 | `lib/protocol/*`, `lib/mux/*`, `attach_registry`, `replay_output`, `session_client`, `remote_socket`, `ports`, `host_*`, `layout/arrangement` |
| Logic tied to the DOM | ~3,600 | `notifications`, `file_offers`, `image_paste`, `window`, `theme`, `terminal_search` |
| Logic tied to dockview or an emulator | ~4,400 | `screen_binding`, `screen_sync`, `screen_snapshot`, `panes`, `close_guard`, `template_apply/extract`, `keyboard_shortcuts` |
| Svelte components | ~7,400 | `Workspace.svelte` (1,023), `Drawer.svelte` (884), `TemplateDialog.svelte` (523), settings sections |

The session protocol core is close to UI-free already. `session_client`, `attach_registry`, `replay_output`, `terminal_session`, the `Presence` class, `Scratchpads`, `drop_upload`, `mux_client`, `incoming`, `screen_startup` and `remote_socket` take their I/O as injected effects or have none.

Three structural problems remain.

1. **Dockview's `DockviewApi` is the client's layout model.** It is the store per screen (`ScreenViews.apis`) and the way to look up a binding (`bindingForApi`). Code asks it whether a pane exists, how many there are and which is active, and reads pane titles (`Drawer.svelte`, `Workspace.svelte`). Launch metadata sits in panel params. The grid comes from `toJSON()`, and the previous snapshot lives in a `WeakMap` keyed by the api.
2. **Orchestration lives in components.** `Workspace.svelte` holds, in its script, the screen lifecycle (start a fresh screen, an emptied screen, move pane to screen, close screen) and server commands. It also applies templates, builds the window title, connects, browses and releases hosts, opens new screens on hosts, prompts for host updates and starts port forwarding.
3. **Domain decisions sit in the window.** They duplicate, contradict or replace the server's. See "Sequence" for where each one goes.

Coupling that blocks UI-free use, besides dockview:

- Svelte runes in `stores/*.svelte.ts`;
- `svelte-sonner` toasts inside logic (`template_apply`, `file_offers`, `editor_requests`, `clipboard_policy`, `remote_editor`);
- desktop shell globals outside `WindowAPI` (`file_offers`, `editor_requests`, `zoom`, `remote_editor`);
- module singletons (`layoutBindings`, the `previousSnapshots` map, registry globals, the `socket` singleton);
- DOM event capture for paste and drop on the emulator's container.

## Goals / Non-Goals

**Goals**

- A client core that a second set of views can drive without the DOM, Svelte, dockview, an emulator or a desktop shell, with that rule enforced by the build.
- Domain decisions made once, in the server, instead of in every client (via the four server changes).
- The client keeps acting first (CC-10). Layout changes show at once, ops go out fire-and-forget, and the server corrects only by rejecting and resyncing. Moving a decision into the server makes it authoritative there; it never makes the window wait where it acts today.
- Dockview, xterm and ghostty-web reduced to renderers.
- Every step keeps `pnpm check`, `pnpm test` and `pnpm build` green and the app usable.

**Non-Goals**

- A native UI, a Rust client core, or a WebAssembly build of Rust code for the browser.
- Transport changes (`native-client-transports`).
- New features, or a protocol change in this change itself.

## Target layering

```
views/     Svelte components, dockview renderers, terminal emulators, overlays
   │  read state, subscribe to events, call commands
   ▼
core/      connections, sessions, screens and screen models, remote hosts, ports,
           settings, notifications, file offers, editor requests, presence
   ▲  implements ports
   │
platform/  browser / Tauri / Electron: HTTP, socket, storage, clock, opener,
           shell bridge, desktop notifications
main.ts    composition root: builds platform, core, mounts views
```

`core/` depends on nothing in `views/` or `platform/`. `views/` and `platform/` depend on `core/` and never on each other. The composition root is the only place where all three meet.

## Decisions

### CC-1: Three layers, one direction

`frontend/src/core/`, `frontend/src/platform/` and the views, which keep their current folders (`components/`, `layout/`, `panes/`, emulator modules) until a later rename. Alternative: a separate package, `frontend/core`, in a pnpm workspace. That is cleaner to publish but adds workspace plumbing without a second consumer yet; move there when the mobile shell exists.

### CC-2: The build enforces the boundary

- `tsconfig.core.json` covers `src/core/**` with `lib: ["ES2023"]` and no DOM library, and `pnpm check` runs it.
- A vitest test fails on imports from `core/` into `svelte*`, `svelte-sonner`, `dockview-core`, `@xterm/*`, `ghostty-web`, `@tauri-apps/*`, `platform/` or a view folder.

Web platform types the core legitimately needs (`AbortSignal`, `Blob` as an opaque file handle, `TextEncoder`, `crypto.randomUUID`) come from a small ambient declaration in `core/` instead of the DOM library. Alternative: ESLint `no-restricted-imports`; the repository has no ESLint.

### CC-3: Plain observable state, adapted in views

The core exposes small stores (`get()`, `subscribe()`) and typed event emitters, without runes. Views adapt them with Svelte 5's `createSubscriber` (`svelte/reactivity`), or a thin `$state` mirror in a `.svelte.ts` adapter. `stores/screens.svelte.ts` already has its own subscribe/notify next to its runes, so dropping the runes there is mechanical. Alternative: runes inside the core, which ties the core to the Svelte compiler and rules out non-Svelte views.

### CC-4: A screen model instead of DockviewApi

Each screen has a `ScreenModel`. It holds:

- the last `ScreenState`: root, floating entries, panes, sessions, jobs, revision;
- the window's pending state: panes waiting for `mux.pane_spawned`, handed-over panes, and the arrangement and active tabs the server is known to have;
- floating-group mapping;
- pane titles and working directories from session events.

The core turns a snapshot into a `layoutReplaced` event with what was added and removed, which sessions to keep and what to reveal, as `screen_snapshot` computes today. Views render the model and report gestures as `reportArrangement` (the dockview adapter normalizes its grid, as `layout/arrangement.ts` does), `selectTab`, `float`, `dock` and `moveFloat`.

What stays window-local in the view: the tab a group shows before it is reported, the focused group, maximize, popout and pixel sizes. The binding registry keyed by api (`bindingForApi`) goes away. Menus, the close guard and keyboard shortcuts get their screen and pane from the model.

### CC-10: The client keeps acting first (Shadow Mode stays)

The model is optimistic, as "Client Autonomy (Shadow Mode)" in `docs/architecture.md` describes today.

- **A command applies at once.** `split`, `newTab`, `closePanes`, `createScreen`, `movePaneToScreen`, `float`, `dock` and `selectTab` change the screen model immediately, with client-generated IDs. The op goes out fire-and-forget.
- **A gesture applies at once.** A gesture the view already performed (a drag, a divider) updates the model from the reported arrangement, without rebuilding the view.
- **The server answers only when needed.** There is no acknowledgement. The server sends:
  - `mux.pane_spawned` for the session to attach;
  - `mux.op_failed` when it rejects an op, after which the core resyncs the screen from the server's snapshot;
  - `mux.nudge` to other windows.

The core keeps today's bookkeeping, so a snapshot that arrives while local changes are in flight does not undo them:

- arrangements and tab selections already sent;
- panes waiting for their spawn;
- handed-over panes.

Server-side decisions from the four server changes follow the same rule. The window renders its intent at once wherever it does today, and only facts the server owns arrive later: titles, busy state, a session's exit.

### CC-5: Terminal sink and session handles

The core's `open(pane, attach | create | awaitSpawn, grid)` returns a session handle with:

- `input`, `reportPaneSize`, `setShown`, `takeOver`, `detach` and `close`;
- events `output` (ordered and replay-filtered), `ptySize`, `metadata` and `closed`.

An emulator implements a sink: `write`, `resize`, `measure`, `cols`, `rows` and an input callback. The view owns the emulator's lifetime and its text such as "[session ended]". Paste, drop and Ctrl+V capture stay in the view and call the core's `upload`, `saveImage` and `pasteFiles`. Today's `TerminalEmulator` interface splits into this sink plus view-only methods (search, selection, theme).

### CC-6: Controllers emit notices, views show them

- `WorkspaceController`: screen lifecycle, moving panes between screens, closing screens, server commands, startup.
- `RemoteHosts`: connect, browse, release, new screen on a host, update offers.
- `Ports`, `FileOffers` and `EditorRequests`.

These controllers replace the script of `Workspace.svelte` and the toast calls inside logic. User-facing results become `notice` events (kind, title, text, optional action) that the views render as toasts or dialogs. Questions to the user (confirm close, approve template commands, confirm host update) are events with a reply, as `close_guard` and `template_guard` already do through listeners.

### CC-7: Platform ports

| Port | Implementations |
|---|---|
| `Http` per connection | `apiFetch`, which already selects the browser gateway or the native API per connection |
| `SocketFactory` | WebSocket or native connection (both exist since `native-client-transports`) |
| `Clock` (now, timers) | real; fake in tests |
| `KeyValue` (small, per window) | `localStorage` with try/catch; in-memory |
| `Opener` (external URL, with a synchronous reservation for user activation) | browser, Tauri, Electron |
| `ShellBridge` (window controls, zoom, remote windows, editor launch, download save/reveal) | Tauri, Electron, none in a plain browser |
| `DesktopNotifications` | Web Notifications API |

Several pieces already follow this pattern: `WindowAPI`, `RemoteWindowsAPI`, the remote helpers that take an injected `fetch`, and the transport selection that `native-client-transports` added (`lib/protocol/connection.ts`, `http.ts`, `native_*`). The core takes these as its ports instead of reaching for module globals. `file_offers`, `editor_requests`, `zoom` and `remote_editor` move behind the shell bridge.

### CC-8: The core stays TypeScript; a native UI gets a Rust core later (provisional, tbd)

Once the four server changes have taken the domain decisions, what remains in the client core is protocol plumbing: routing, attach, replay filtering, presence, startup ordering, reconnect, and the screen model. A native UI would port that to a Rust crate seeded by `splitty attach`'s session client (`cli/attach`), which already filters replays, redraws after `attached`, takes over on welcome and throttles activity. The duplication is bounded and specified by `mux-protocol` and `terminal-sessions`.

Alternative: one Rust client core compiled to WebAssembly and shared by both UIs. That gives one implementation, but adds a WebAssembly boundary for every event, async and socket bindings, a larger bundle, startup cost and harder debugging. It also works against the startup-time goal a native UI would have.

### CC-9: Older servers degrade instead of keeping old logic (provisional, tbd; applies to all five changes)

When a server lacks the capability a moved decision depends on, the client does not keep a copy of the old client-side logic. The feature degrades instead:

- applying a template is unavailable, and the host's update is offered;
- a screen without a name shows its id;
- titles fall back to the process name the session reports.

Copies stay only where the fallback is a line or two. The local server always has the app's version; only hosts with older daemons are affected, and the hub already offers their update. Alternative: keep the old logic in a `core/legacy/` module until a cut-off version, which keeps the duplication this work removes.

## Sequence

| # | Change | Moves out of the window | Server side |
|---|---|---|---|
| 1 | `server-templates` | `template_apply` and `template_extract` (about 470 lines), "apply in new screen" with its throwaway pane | template → arrangement plus launch data; a mux op that applies it; extraction from the server's screen |
| 2 | `server-derived-pane-state` | default screen names, the pane display title, the "busy" part of the close guard, close on exit, launch defaults for new tabs, the replay filter | reducer names; display title in session metadata; a busy flag; exit → `close_pane` in the effect runner; replay filtered at the source |
| 3 | `settings-authority` | settings defaults and limits, editor preference sanitizing, default shell profile resolution | defaults and limits from the server, full validation, a settings change event, a profile fallback with a reason |
| 4 | `hub-host-decisions` | update availability, retry classification, host directory merge, catalog writes, port polling | `update` in `remote.ready`, `retryable` in `remote.error`, a host directory read, hub-written catalog, a forwards change feed |
| 5 | `frontend-client-core` | — | none |

Changes 1 to 4 are independent of each other and can be built in parallel. Each adapts the current frontend to its new server behavior; change 5 then extracts a core without them. The other order would build core adapters for logic that is about to disappear.

What stays client-side on purpose, as window-local state or because a spec says so:

- the consent step for template commands (SR-12);
- remembered remote screens (`remote-screens`, Remembered Remote Screens);
- automatic port forwarding, which would make the hub parse relayed host data (architecture.md, "Remote Screens");
- notification display, read state and per-type filters;
- renderer choice, zoom, theme application;
- the editor URI construction, which `access-control` places in the trusted shell.

## Inventory

By area, at 2026-11-04. C = client core, W = window-local by design, D = domain logic for the server, V/G = view and glue.

**Workspace and layout**

- `mux_client`, `incoming`, `screen_startup`, `arrangement`, `job_snapshot`, `ids`: C, UI-free already.
- `screen_binding`: C 60 / G 40. `ScreenBinding` is core; the api registry and `bridgeDockviewToMux` are glue.
- `screen_snapshot`: C 35 / G 65. The diff, sessions to keep and active-tab bookkeeping are core.
- `panes`: C/G/D/W mixed. Its launch defaults contradict "Split Session Inheritance" for new tabs.
- `close_guard`: D 50 / W 30 / C 20.
- `template_apply` and `template_extract`: D/G.
- `Workspace.svelte`: about 35 % C, 30 % G, 25 % remote orchestration, 10 % V.
- `Drawer.svelte`: V, but it reads panes and titles from `api.panels`.

**Remote, connections, transport**

- Mostly C with injected effects: `remote_socket`, `ports`, `host_order`, `host_preferences`, `screen_groups`, `remote_screen_sync`, `protocol/*`.
- D parts:
  - `update.ts` treats any other version as an update;
  - `on_hosts` and `host_directory` repeat the server's known-host merge;
  - `host_directory` writes the catalog;
  - `port_forwarding` polls and auto-forwards;
  - `remote_socket.retryable` re-encodes the hub's error codes.
- W: `remembered`, `close_screen`, `remoteWindows`.

**Terminal, sessions, settings, chrome**

- C, UI-free: `session_client`, `attach_registry`, `replay_output`, `terminal_session`, `drop_upload`, `managed_job_mount`, the `Presence` and `Scratchpads` classes.
- D:
  - settings defaults and merge, which disagree with the server: `sidebarWidth` is 240 in the client and 200 on the server, slider bounds differ from `validate`, and fractional scroll sensitivities are sent for integer fields;
  - default profile resolution;
  - the pane title rule;
  - close on exit;
  - editor preference sanitizing.
- V: the emulators, overlays, settings sections.
- W: the window and platform helpers.

**Viewer panes** (added on 2026-11-06 by `splitty-open` and `review-pane`)

- C, UI-free: the document reader (`lib/viewer/documents.ts`), the choice of presentation (`presentation.ts`), the annotation store, anchoring and the review (`annotations.ts`, `anchoring.ts`, `review.ts`).
- V: the source view (CodeMirror), the rendered view in its shadow root, the annotation list, form and selection toolbar.

Findings that are bugs or dead code are listed in `docs/issues/openspec-baseline-findings.md` ("Client layering", 2026-11-04).

## Architecture impact

- `architecture/landscape.c4`: the `web` container's description names its layers (core, platform, views) and the rule that the core uses no UI or shell API. No components are added for the web container: `architecture/check` maps components to Rust modules, and extending it to TypeScript is out of scope. CC-2 enforces the frontend's boundary instead.
- `docs/architecture.md`: a new "Client layers" section. Frontend paths in "Client Autonomy", "Start-up and reload", "Attaching to sessions", "PTY Size", "File Transfers" and "Remote Screens" are updated to the core modules.
- No change to the Rust components or their relationships in this change. The four server changes state their own impact.

## Migration Plan

Within this change, each step merges green:

1. **Skeleton.** `core/`, `platform/`, `tsconfig.core.json`, the import test. Move the modules that are already UI-free: protocol, mux, attach, replay, session client, remote socket, ports, host helpers, scratchpads, presence.
2. **Sessions.** Session handles and the terminal sink. Emulator mounting becomes view code; paste, drop and upload go through the core.
3. **Remote.** Remote host and port controllers; their orchestration leaves `Workspace.svelte`.
4. **Screens.** Screen model and `WorkspaceController`. Dockview becomes a renderer, and the api registry is removed. Keyboard resize and floating bounds are reported.
5. **The rest.** Settings, notifications, file offers and editor requests without toasts, runes or shell globals.
6. **Cleanup.** Dead code, docs, architecture description.

## Risks / Trade-offs

- **Startup, attach and snapshot sequencing is subtle.** It is protected by today's vitest suites, which move with their modules. New tests for the core run without a DOM.
- **Parallel work on new pane kinds conflicts with step 4.** `review-pane` and `splitty-open` touch the same dockview code. They should land after step 4, or render from the screen model from the start.
- **Reactivity adapters can re-render too much.** Stores emit fine-grained changes per screen and per session, not one global change.
- **The server changes depend on host updates (CC-9).** Hosts on older daemons lose features until updated, rather than keeping old client logic.

## Provisional decisions

Taken on 2026-11-04 to keep going, open to the maintainer's feedback; each also has a bullet in `openspec/open-decisions.md`:

- **CC-1** `core/` and `platform/` inside `frontend/src`, not a separate package yet.
- **CC-2** boundary enforced by a DOM-free TypeScript project and an import test.
- **CC-3** plain stores and events in the core, Svelte adapters in views.
- **CC-8** TypeScript core now, a Rust core for a native UI later; no WebAssembly sharing.
- **CC-9** older servers degrade instead of the client keeping moved logic.
- **Sequence** server changes 1–4 before the core extraction; what stays client-side as listed under "Sequence".
