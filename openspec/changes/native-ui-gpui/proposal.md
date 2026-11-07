# Proposal

The bounded Linux phase-0 implementation is recorded in [the spike report](../../../docs/gpui-spike.md)
(`native-ui-gpui-spike`). Platform and input checks in that report remain gates for
the full migration described below.
Completing that experiment does not establish platform parity or authorize the
release cutover described below.

> Draft captured on 2026-11-06. Not planned yet. Specs, design and tasks are written when this change is next, after the spike (phase 0 below). Depends on: steps 1 to 4 of the `frontend-client-core` sequence (`server-templates`, `server-derived-pane-state`, `settings-authority`, `hub-host-decisions`), which move domain decisions out of the window into the server and so shrink what a native client has to port; step 5 of that sequence, the TypeScript client core, is replaced by this change's Rust client core (N2). Builds on `native-client-transports` (the native client connection and transport crate the new client uses), `client-connections`, `mux-protocol`, `terminal-sessions`, `remote-screens`, `desktop-remote`, `viewer-panes`, `viewer-annotations` and `access-control`. Related: the viewer drafts `viewer-renderers`, `viewer-quick-edit`, `review-diffs`, `viewer-sandboxed-html` and `viewer-webview`, whose renderers this change has to provide; the open decisions of `arrange-panes` and `remember-selected-tab`, which exist because the web layout library owns the window's layout; the open decisions of `mobile-ssh-client` (M1–M3), which assume a Tauri mobile shell with xterm.

## Why

The desktop app is a web application: Svelte 5 and TypeScript in an OS WebView (Tauri) or in Chromium (Electron on Linux), with xterm.js or ghostty-web for terminals and dockview for the layout. That costs more with every feature:

- **Two models of one layout.** dockview owns the window's layout, the server owns `ScreenState`. They are reconciled after the fact: `arrange_panes` 100 ms after a gesture, `select_tab` for tab clicks, snapshot rebuilds that suppress ops. Drift between them is a known class of bugs (`docs/layout-sync-drift.md`).
- **A renderer that must be treated as hostile.** Terminal output, documents and data from hosts end up in a WebView that runs the app's script and holds its IPC, so the shells carry document proofs, nonces, navigation guards, capabilities, packaged-asset allowlists and per-window storage isolation (about 3k lines in the Tauri shell, plus the Electron shell), and every rendered document has to be sanitized into a shadow root of that same page.
- **A separate browser runtime on Linux.** WebKitGTK compatibility problems led Linux releases to use Electron; the native experiment must establish whether one rendering stack can replace that runtime across platforms.
- **Protocol handling in two languages.** The server has Rust types and validation; the client has hand-written types and shape checks (`docs/protocol.ts`, `types.ts`, `incoming.ts`, `untrusted.ts`).
- **Browser-era workarounds in the client.** Ctrl+V goes through the browser's paste event because the page may not read the clipboard; OSC 8 links are confirmed in a Splitty notice because a WebView answers `confirm` with false; downloads go through `blob:` URLs; zoom is page zoom; a native host stream is wrapped as a WebSocket so the browser's state machine can drive it.

A native client in Rust removes the WebView boundary around the app and the layout reconciliation, shares protocol types and the pure reducer with the server, and needs one renderer per platform. GPUI fits the app: a GPU-rendered UI framework built for an editor with an integrated terminal, panes, previews and a command palette, with an action and keymap model that matches the routing of keys between a terminal and the app's shortcuts.

## What Changes

- **A native desktop client on GPUI** replaces the web UI in the desktop app on Linux, macOS and Windows. Local sessions keep the app-owned embedded service (in-process on Linux and macOS, the WSL helper over inherited pipes on Windows); hosts keep the native SSH path.
- **A client core in Rust without UI**: connections and per-connection routing, the mux client and screen sync, the session client with attach handling and presence, the host controllers (hub connection, forwards, reverse forwards, remembered screens, host directory), the viewer's document reads and annotation state with anchoring by quote, notifications, templates and settings access. It knows neither GPUI nor transport details beyond the native client connection, and is tested in-process against the real embedded service.
- **The window renders the server's layout.** The dock edits `ScreenState` through ops instead of reconciling a second layout model, and may apply an op optimistically with the server's own reducer.
- **A native terminal view** on a replaceable Rust-usable terminal emulator, with selection, search, links and OSC 8 handling, the clipboard policy, IME, the PTY size marker and the scratchpad area. An engine adapter isolates emulator-specific state and types from the GPUI renderer, workspace and client core; changing that adapter must not change session ownership or layout logic.
- **Viewer panes render natively** (N5): Markdown, diagrams, SVG, tables, source views with syntax highlighting, diffs and quick edits, with annotations marked on the client's own text layout. A web engine is embedded only for content that runs code, scripted HTML (`viewer-sandboxed-html`) and dev-server pages (`viewer-webview`): rendered offscreen into an element of the window, so overlays, clipping and scrolling work as for any other pane, and holding no authority (no channel to the service, at most a narrow data channel for selections and quotes).
- **Browser access ends with the web UI.** At cutover, `splitty serve`, the browser gateway with its token, session cookies, bootstrap codes and tickets, and the browser part of `splitty connect` are removed. Until then they stay: the web UI is tested headless through them (N4).
- **BREAKING (at cutover):** no browser access. Terminal access without the app remains `splitty attach`, also over SSH.

Phases:

0. **Spike**, on Linux (Wayland and X11), macOS and Windows, each part attached to the embedded service through the native client connection:
   - a terminal pane: text quality, scrolling under sustained output, IME and dead keys, emoji and fallback fonts, selection and clipboard; it decides the terminal emulator;
   - a Markdown viewer pane with annotation marks on its rendered text;
   - an offscreen web engine in a pane, under an overlay, on Wayland in particular.

   It decides whether the plan holds and when the cutover is planned (N3). The completed bounded probe establishes functional data paths and composition, but does not pass an interactive-performance gate. Before expanding its fixed layout, measure idle CPU, input-to-paint latency, sustained output and scrolling in an optimized build, with CEF disabled and enabled, recording the actual graphics device. Use those measurements to remove unnecessary redraws and repeated text work; a successful smoke is not a performance result.
1. **Client core**: connection model, mux sync, sessions, viewer documents and annotations, ported with their test cases; no UI.
2. **Local desktop**: windows, the dock on `ScreenState`, the terminal view, the local workspace, packaging next to the existing apps.
3. **Hosts**: remote screens, ports and reverse forwards, the host directory and remembered screens.
4. **Remaining features**: viewer panes with annotations, threads and reviews; settings, dialogs, command palette, context menus, notifications, templates, scratchpad, editor handoff, downloads and file offers, zoom.
5. **Cutover**: parity checked against the specs; releases ship the GPUI app; the web client, the Electron shell, the WebView parts of the Tauri shell and the browser gateway are removed and the specs updated.

Out of scope:

- Mobile clients. The client core should be usable by one later; GPUI's iOS platform is experimental and no mobile shell is part of this change.
- A web build of the new client (GPUI's experimental web platform) or any other replacement for browser access.
- A feature freeze of the web client before the spike (N3). Until the cutover is planned, new features keep their logic on the server or in pure functions, as the viewer's anchoring does, so that it carries over.
- Changes to the mux model, session keepers, the CLI's HTTP API over owner and scoped sockets and the native transport, except where the native client needs them; wire changes only with a capability, because hosts run older daemons.
- Screen-reader accessibility beyond what GPUI provides (open question).

## Architecture gates before the local desktop

- **Independent client core:** connection identity, ordered session traffic, replay state, mux synchronization and owned-resource cleanup remain independent of GPUI and the emulator. The service continues to own PTYs and authoritative workspace state. A view or rendered document never selects a backend or acquires service authority.
- **Replaceable emulator:** keep parsing, terminal modes, scrollback and engine-specific selection operations behind a small internal adapter. Expose engine-neutral visible text/style data, cursor, change information and checked input/reply operations. No Alacritty types may cross into the GPUI renderer, workspace or client core. Capability differences must be explicit; replacement does not imply identical image/keyboard/Unicode support or require a runtime plugin system.
- **Separate native presentation:** GPUI owns text shaping/painting, focus, pointer geometry and IME composition. Clipboard actions and replay-triggered effects remain subject to application policy. Changing the emulator must not require rebuilding workspace, transport, clipboard policy or platform-lifetime logic. Rendering must not copy the entire scrollback or rebuild unchanged rows on every frame.
- **One layout authority:** the first usable desktop increment renders the service's screen layout and sends split, resize, tab and close intents through the existing mux operations. Its state model must not grow a second independently authoritative dock tree. The current 55/45 terminal/document fixture is not a layout implementation.
- **Explicit scheduling and ownership:** parsing, UI updates and optional web rendering need measured execution budgets and bounded queues. Idle state must not force whole-window redraws. Frame/document/session ownership, cancellation and shutdown must remain explicit across worker and UI boundaries.
- **Enforced boundaries:** extend the LikeC4 model and import checks with the engine-adapter/rendering separation when implementing it. Test contracts across that boundary: ordered replay/live bytes, resize, selection, terminal replies, connection identity and teardown. Performance evidence covers idle, typing, output and scrolling separately, with environment and build profile recorded.

The immediate implementation order is performance measurement and emulator separation, followed by the client-core work needed for real service-driven splits/tabs. The full migration remains a draft until its design, behavior deltas and tasks are written.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- None expected. Requirements of UI parts stay limited to what crosses the protocol or an API and to UX concepts not evident from the built app.

### Modified Capabilities
- `client-connections`: "Native Document Lifetime" and the document-bound handle rules become rules for a native window; WebView documents, reloads and the popout trust domain go.
- `access-control`: REMOVED deltas, with their reason, for the browser requirements (Origin Allowlist, Fetch Metadata Same-Origin Rule, Process Token, Browser Session Establishment, Single-Use Bootstrap Codes, WebSocket Ticket Authentication, Browser UI Holds No Credential, Browser Gateway Credential Isolation) and for the WebView-specific window rules. "Rendered Documents Are Inert" is restated for native rendering and for an embedded engine without authority. The rules on untrusted host data, per-connection routing, viewer documents staying with their connection and server commands acting only on the sender's screens stay.
- `server-modes`, `cli`: browser gateways, `splitty serve`, `splitty connect`.
- `desktop-remote`: whether separate remote windows remain once remote screens work on every platform.
- `remote-screens`: possibly where the hub runs (open question).
- `mux-protocol`: the role of `arrange_panes` and `select_tab` once the client edits the layout with precise ops.
- `terminal-sessions`: paste and image paste without a browser paste event, OSC 8 confirmation, uploads as binary data (only with a capability).
- `viewer-panes`, `viewer-annotations`: probably none. "Showing A Document" requires rendered HTML documents, which a native client meets with the static-HTML renderer (open question); reads, anchoring by quote, threads and reviews are protocol and stay. "Rendered Documents Are Inert" in `access-control` names Splitty's origin and `frame-src`, which only a browser client has.
- `display-zoom`: zoom as UI scale and font size instead of page zoom; the browser clause goes.

## Impact

- New crates: the client core and the GPUI desktop app. The architecture model gains them with their allowed dependencies (client core: protocol types of splitty-core, the native client of the server crate, splitty-remote; desktop app: client core, GPUI, the embedded web engine).
- Removed at cutover: `frontend/`, `frontend-electron/`, the WebView parts of `crates/splitty-tauri` (the IPC carrier, document sessions and bootstrap, navigation, capability, downloads), the browser gateway in `crates/splitty-server/src/http` (gateway, browser carrier, browser sessions, the browser listener, origin and fetch-metadata middleware), the process token in splitty-core, and the Playwright and WebView smoke scripts in `scripts/testing/`. The parts of the Tauri shell that are not about the WebView move to the new app: WSL helper management, the login PATH, editor handoff, the remote connect flow.
- `frontend-client-core`: step 5 is dropped when this change goes ahead (N2); its design's inventory, ports, screen model, terminal sink and controllers carry over to the Rust core.
- Packaging and release: GPUI app bundles replace the Tauri and Electron packages; an offscreen web engine such as CEF adds a three-digit number of megabytes and its own security updates; the release skill and CI change.
- Docs: `docs/architecture.md` (Client Autonomy, Client connections, the Tauri sections, the viewer), `docs/networking.md`, `docs/development.md`, `docs/protocol.ts` for any wire change, `README.md`.
- Tests: the vitest suites are the reference for the client core's Rust tests; GPUI's test context covers the UI in-process.

Open questions for the design:

- Terminal emulator: alacritty_terminal (mature, used by Zed; no image protocols) or libghostty-vt (the same core as the ghostty-web renderer shipped today; the maturity of its native API is unknown). Decided by the spike.
- Web engine for scripted HTML and dev-server pages: CEF rendered offscreen (uniform on every platform; size and updates), Servo (MPL-2.0, experimental embedding), or none, with those pages opened in the system browser. System WebViews as child windows are not an option for panes: nothing can be drawn over them (menus, notices, the dock's drop targets), they are not clipped, and on Linux they embed only under X11.
- Static HTML documents: a native HTML and CSS renderer without scripts (Blitz, in beta and not yet integrated with GPUI) or the web engine.
- PDF without pdf.js: a native PDF renderer.
- Widgets and dock: gpui-component or own components; which GPUI release to pin (see the notes).
- Where the hub runs: in the local service as today, or in the app process with splitty-remote as a library. The latter gives one path to hosts on every platform, with or without WSL, and may make separate remote windows unnecessary.
- How the client calls the service: HTTP-shaped native requests (`docs/native-operations.json`: method and path, status codes, bodies as strings) or typed operations. HTTP stays for the CLI either way.
- Optimistic reduce with the shared reducer, or ops with a short wait for the snapshot; how a host whose server version differs is handled (its snapshot is the truth).
- What `splitty connect` becomes: removed, or "open this host in the desktop app".
- Whether browser access leaves release builds before the cutover (a cargo feature for development and tests), for a smaller attack surface in the meantime.
- Accessibility.
- The development loop without hot reload: rebuild time of the desktop crate, and keeping the client core's tests free of UI so they stay fast.

## Carried-over design notes

- 2026-11-07, post-spike architecture review: Alacritty imports are confined to `terminal.rs`, but that GPUI view directly owns its parser/model and consumes its cell, selection and mode types. It is isolated from the service, not yet independently interchangeable with the renderer. The probe requests redraws every 16 ms, shapes/paints nonblank cells individually and parses output on the UI thread. These are concrete optimization candidates, not measured attribution of latency. The preview used an unoptimized build on software Vulkan; optimized measurements and the architecture gates above precede further feature expansion.

- 2026-11-07, `native-ui-gpui-spike`: the native adapter, isolated service lifecycle, terminal and Markdown annotation path now exist as a separately invoked development executable. Reconnect retains the same service session. The full host/workspace/settings client is still future work. Windows has an explicit connection adapter but no runnable owned-helper launcher; no local Windows or macOS runtime evidence exists. See the [evidence matrix](../../../docs/gpui-spike.md) before extending the migration.
- 2026-11-07, dependency correction from `native-ui-gpui-spike`: GPUI Kit 0.9.0 pins the `gpui-pre`/platform 0.3.11 snapshot, which uses wgpu on Linux. Published `gpui` 0.2.2 uses Blade; those versions must not be described as the same renderer. The probe chooses `alacritty_terminal` 0.26.0 provisionally and implements its own native terminal view. Native Linux screenshot capture is absent in this pinned GPUI platform; runtime logs and external window captures are distinct evidence.

Input for `design.md`:

- 2026-11-06, inventory of the web client (production code, without tests): 26.8k lines of TypeScript, of which 8.1k belong to the viewer (documents, rendering, sanitizing, anchoring, annotations, threads and reviews); 7.7k of Svelte (of which about 930 lines of `Workspace.svelte` are script: orchestration rather than markup); 3k of CSS; 19.2k lines of vitest tests. Roughly:
  - about 3.2k lines of TypeScript are web wiring and have no counterpart: the transport and bootstrap adapters in `lib/protocol/` (Tauri and Electron channels, native socket and connection, the HTTP-shaped API adapter, WebSocket, browser assignment and login), the shape checks and types (`incoming.ts`, `types.ts`), the dockview bridge (`arrangement.ts`, `screen_snapshot.ts`, the tab, group, watermark and new-tab renderers, auto-hide tabs, parts of `screen_binding.ts`), `replay_output.ts` (the same rules exist in Rust as `pty::replay`), DOM helpers;
  - about 2.7k lines become the native terminal view: the xterm and ghostty adapters, the terminal pane, search, the PTY size marker, scroll-to-bottom and the scratchpad area;
  - about 11.5k lines are client logic that is ported, not dropped: connections and mux (`lib/connection/`, `lib/mux/`), sessions and attach (`session_client`, `attach_registry`, `terminal_session`, `presence`, image paste and uploads), the host controllers (`lib/remote/`, 3.4k lines), notifications, scratchpad, settings, templates, close and template guards, context menus, keyboard shortcuts, editor handoff, file offers;
  - of the viewer, the document reads (`documents.ts`), the annotation state and rules (`annotations.ts`), anchoring by quote (`anchoring.ts`, pure functions over strings), the review and its hand-over (`review.ts`, `handover.ts`) are client logic and ported; rendering, sanitizing into a shadow root, CSS highlights, the CodeMirror source view and the annotation layer's DOM are rebuilt natively;
  - the Svelte components and CSS are rebuilt.
  Most of the client logic exists because the server leaves decisions to clients by design (client-generated IDs, attach handling, presence, host control), not because of the web. Annotations, threads, reviews, `splitty review` and the agents' replies already live on the server.
- 2026-11-06, approach (N1, provisional, tbd): a rebuild next to the web client, not a cleanup first. The web code is not refactored before the migration: what the native client does not take over is deleted at cutover. The native client takes over explicitly:
  - the Rust crates (core, server, transport, remote) as dependencies, unchanged unless the native client needs something (for example `Deserialize` for server events, typed service operations);
  - the non-WebView parts of the Tauri shell, moved rather than copied;
  - the client logic, ported module by module along the specs and the protocol. The TypeScript is a reference for edge cases (its comments say why), not a template; each module's vitest cases are ported as Rust tests first.
- 2026-11-06, relation to `frontend-client-core`: its steps 1 to 4 move templates, derived pane state, settings authority and host decisions into the server; every line moved there is a line the native client does not port, so they come first either way. Its step 5 extracts a TypeScript core with ports, a screen model, a terminal sink and controllers that emit notices; its own CC-8 already expects a Rust core for a native UI, seeded by the session client of `splitty attach`. Building both would port the same logic twice (N2).
- 2026-11-06, web-era artifacts not to carry over into the new architecture:

  | Today | Why it exists | In the native client |
  |---|---|---|
  | `arrange_panes` after a 100 ms wait, `select_tab` rules, ops suppressed during snapshot rebuilds | dockview owns the layout and does not know the server's node IDs | the dock edits `ScreenState` with precise ops; `arrange_panes` only for compatibility, if at all |
  | HTTP over IPC: replies with status code, content type and a string body; `docs/native-operations.json` maps method and path to operations | the client API came from the browser | typed operations at the service; HTTP stays for the CLI |
  | uploads, `save_image` and viewer document chunks as base64 inside JSON | JSON over WebSocket | binary data on the native connection, behind a capability |
  | two paths to hosts: the hub in the local service for remote screens, the native broker's own SSH for remote windows | a browser cannot run SSH; Windows without WSL has no local service | possibly one path: splitty-remote in the app process, remote screens in the main window on every platform |
  | document proofs, nonces, navigation guards, reload revocation, popout trust domain, incognito storage for remote windows, `inert` on inactive screens | script runs in the WebView | gone; focus decides which terminal gets keys |
  | documents sanitized into a shadow root of the app's own page, links stripped of `href`, CSS Custom Highlight marks | the document shares the app's page and its authority | native rendering, or an engine without authority for pages that run code |
  | Ctrl+V through the paste event, clipboard reads forbidden | browser permission model | the client reads the clipboard |
  | OSC 8 confirmation in a notice, toasts for blocked popups, reserved external opens | WebView `confirm` returns false; browsers block late popups | the system opener; whether links still ask first is a UX decision of its own |
  | downloads through `blob:` URLs | the page cannot write files | the client writes the file |
  | page zoom | browser zoom | UI scale and font size |
  | `NativeHubSocket` posing as a WebSocket | the hub's state machine was written for the browser | the hub connection as a native stream |

- 2026-11-06, protocol design that is not a web artifact and stays: client-generated pane and screen IDs; ops without acknowledgement and `mux.op_failed` for rejections; snapshots and nudges for other clients; the attach registry (a server streams a session at most once per connection); the replay filter; the pane's size and the PTY's size kept apart, with `activity`, `idle` and `hide`; resolving every message only against the objects of the connection it arrived on; text from hosts as plain text with length caps; viewer documents read in versioned chunks over the pane's own connection; annotations anchored by quote with context and source lines.
- 2026-11-06, GPUI ecosystem as checked on that day: `gpui` 0.2.2 on crates.io (Apache-2.0), with a Blade renderer (corrected by the 2026-11-07 spike; the newer pinned snapshot uses wgpu), Wayland and X11, Windows and macOS, a tokio bridge (`gpui_tokio`, Apache-2.0) and experimental web and iOS platforms. Zed's own crates that would be the obvious templates (`terminal`, `terminal_view`, `workspace`, `ui`, `markdown`, `markdown_preview`, `mermaid_render`) are GPL-3.0-or-later and must not be copied into this MIT project; they can only be read. gpui-component 0.9.0 (Apache-2.0) has a dock, text inputs, a text view with Markdown, simple HTML and range highlights, and a tree-sitter highlighter, but pins `gpui-pre =0.3.11`, a republished snapshot, so an app using it is tied to that snapshot. Discord announced on 2026-11-04 that its desktop client moves to GPUI, with its own patches on top. The API follows Zed's needs; versions are pinned exactly and upgraded deliberately.
- 2026-11-06, previews and web engines for GPUI as checked on that day: Zed embeds no web engine and has no HTML preview; it renders previews natively (Markdown with pulldown-cmark and an HTML subset through html5ever, Mermaid through the `merman` crate, SVG through resvg, images, tabular data). Those libraries are MIT or Apache-2.0. For a real engine there are only third-party integrations: `gpui-webview` in gpui-component (wry; WebKitGTK in an X11 child window on Linux, since GTK cannot embed into a foreign Wayland surface; macOS; no Windows), `gpui-cef` (CEF rendered offscreen as a GPUI element on macOS, WebView2 as a child window on Windows, no Linux), an experimental Servo web view for Wayland in the `gpui-fusion` fork, `servo` 0.7.0 (MPL-2.0) and the `cef` crate 156 (Apache-2.0 or MIT) on crates.io, and Blitz 0.3 beta (HTML and CSS without scripts, MIT or Apache-2.0), integrated with iced but not with GPUI.
- 2026-11-06, testing: GPUI's test context runs the UI without a window or GPU, with simulated keystrokes, actions, a simulated clock and reproducible randomized scheduling. With the client core in Rust, a UI test can run the real embedded service with real PTYs in the same process, at the in-process level of CLAUDE.md. What is lost is the black-box run against the built app in a headless browser; a test-only control channel in the binary, run under Xvfb as the WebView smokes are today, would replace it where needed.
- 2026-11-06, provisional decisions, each also in `openspec/open-decisions.md`: N1 rebuild next to the web client with one cutover, no cleanup first; N2 a Rust client core without UI beside the GPUI app crate, replacing step 5 of `frontend-client-core`, after its steps 1 to 4; N3 the cutover is planned after the spike, without a web feature freeze before it; N4 browser access stays in release builds until the cutover and goes with the web client; N5 viewer formats render natively, and a web engine, rendered offscreen and without authority, only for pages that run code.
