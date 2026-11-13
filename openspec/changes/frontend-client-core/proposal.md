# Proposal

> Review note, 2026-11-07: this is a historical TypeScript extraction design. The four service-owned increments are implemented, and an isolated native Rust client now exists. TS-only extraction, shared portable Rust client logic with a browser adapter, and retaining separate presentation adapters are being evaluated; none is selected by this note. See the `native-ui-gpui` proposal's current architecture review. Do not interpret this draft as requiring either two independent feature cores or immediate web removal.

> Draft created on 2026-11-04. Specs and tasks are written when this change is next. It comes last in a sequence of five: `server-templates`, `server-derived-pane-state`, `settings-authority` and `hub-host-decisions` first move domain decisions out of the window into the server, so this change extracts a core that holds no service-owned domain decisions (design, "Sequence"). The design records the inventory of `frontend/src` and the target layering for all five. Related: `native-client-transports` (merged; its browser and native transport adapters become the core's HTTP and socket ports), `mobile-ssh-client` (its mobile workspace renderer is a second set of views over this core), `review-pane` and `splitty-open` (new pane kinds render from the screen model introduced here).

## Why

The server is the source of truth for workspaces and sessions. The web frontend nevertheless mixes three kinds of code that should be kept apart:

- views: Svelte, dockview and the terminal emulators;
- logic every client needs: connections, attach, PTY-size presence, startup ordering, reconnect;
- decisions that belong to the server: how a template becomes a layout, default names, pane titles, settings defaults, whether a host update is available.

An inventory of `frontend/src` (2026-11-04, see the design's "Inventory") found:

- About 7,600 of the 23,000 non-test lines are already UI-free. The session protocol core is nearly clean.
- Dockview's `DockviewApi` acts as the client's layout model. Whether a pane exists, its title, its launch data (kept in panel params), the grid and the previous snapshot are all read from it.
- Screen lifecycle and remote host orchestration live in the `<script>` block of `Workspace.svelte`, about 930 lines.
- Domain decisions in the window duplicate or contradict the server, which has caused bugs:
  - an applied template's layout never reaches the server;
  - a host running a newer version is offered an "update";
  - settings defaults and limits disagree with the server's validation.

A second set of views, such as a mobile renderer (`mobile-ssh-client`) or a native desktop UI, needs a client core free of the DOM, dockview, Svelte and any desktop shell. Today such views would have to re-derive the behavior from the existing views.

## What Changes

- **UI-free client core** in `frontend/src/core/`. It holds:
  - connections and transport;
  - session handles;
  - the window's screens, with one model per screen;
  - controllers for remote hosts and ports;
  - settings, notifications, file offers and editor requests.

  It imports no DOM types, Svelte, dockview, terminal emulator, toast library or desktop shell API. Two checks enforce this: a TypeScript project of its own without the DOM library, and an import test.
- **Platform ports**, supplied to the core by the composition root: HTTP and socket per connection; clock and timers; small key-value storage; external opener; desktop shell bridge (window, remote windows, editor launch, downloads); desktop notifications. The browser, Tauri and Electron implementations live in `frontend/src/platform/`.
- **Screen model.** A screen model holds the last snapshot and the window's pending state. Dockview renders it and reports gestures as an arrangement or a tab selection. Nothing reads `DockviewApi` as the source of truth for panes, titles or launch data any more.
- **The client keeps acting first (Shadow Mode stays).**
  - Every layout command and gesture changes the model and the view at once.
  - Ops go out fire-and-forget.
  - The server corrects only by rejecting (`mux.op_failed`), after which the window resyncs.
  - Nothing that is optimistic today waits for the server afterwards (design CC-10).
- **Terminal sink.** Terminal emulators implement a narrow sink: write, resize, measure, input. Paste, drop and file capture hand their files to the core.
- **Controllers instead of `Workspace.svelte`.** Screen lifecycle and remote orchestration move into core controllers. User-visible notices become core events that the views show.
- **Fixes along the way**, from the inventory, in code this change moves:
  - keyboard resizes reach the server;
  - floating bounds are reported;
  - one neighbor search, the server's;
  - one upload batch runner, so dropped and pasted paths are quoted the same way;
  - dead code removed;
  - no per-message logging in production.
- No change to the wire protocol, and no change in behavior beyond those fixes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None expected. UI parts are specced only where they cross the protocol, and the fixes restore specced behavior. Whether the change sets `skip_specs` is decided when it is next.

## Impact

- **Frontend:** most of `frontend/src` moves or is split.
  - `pnpm check` also checks the core project.
  - Tests move with their modules.
  - Core tests run without a DOM environment.
- **Desktop shells:** their preload and IPC surfaces stay as they are. The frontend reaches them only through `platform/`.
- **Architecture:** the `web` container's description names its layers, and `docs/architecture.md` gains a "Client layers" section with updated frontend paths (design, "Architecture impact").
- **Server, protocol, CLI:** no change.

Out of scope:

- a native UI, and a Rust client core (design CC-8);
- transport changes (`native-client-transports`);
- remembered remote screens as server state;
- notification filtering on the server;
- extending the architecture check to TypeScript.

## Carried-over design notes

- 2026-11-10, follow-up to the `native-ui-gpui` architecture review: retain the current crate structure, clarify responsibility boundaries, and reduce duplicated Web/GPUI client logic through shared client-core policies. Review the scope and naming of `splitty-remote`; the concrete browser reuse mechanism remains open.
- 2026-11-13, `remove-electron-shell`: the Electron shell (`frontend-electron/`) is removed. What this change says about Electron, its main process, preload or packaged assets no longer applies; the desktop shell around the web frontend is Tauri, and the native app (`crates/splitty-gpui`) is the default desktop app.
