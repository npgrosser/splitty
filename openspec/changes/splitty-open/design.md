# Design

## Context

- **Panes without a session already fit the model.** `PaneState.session_id` is optional, `PaneRole` has `Viewer`, `close_pane` emits `KillSession` only for a pane with a session, and `restore_effects` skips panes without one. No operation creates such a pane, and every reducer path sets the role `terminal`. The frontend registers only the `terminal` dockview component and rebuilds every snapshot pane as a terminal, ignoring the role on purpose since web-app panes were removed (SR-01).
- **Other windows learn about sessionless changes late.** Over the WebSocket, a nudge to the other clients follows only ops with effects, plus `move_pane_to_screen`, `set_scratchpad` and `arrange_panes`; the HTTP path nudges after every accepted op.
- **Files already cross the connection**: chunked uploads (1 MiB per message), image paste, and file offers read in 1 MiB chunks (`file.read`/`file.chunk`). Native carriers, the hub relay and the native relays admit only the session message types in `SESSION_OPERATIONS`; the plain WebSocket does not check.
- **`splitty open`** hands a file or folder to VS Code or Zed on the client after a confirmation in the window. `--in splitty` was reserved for this change. The CLI refuses `open` under an agent grant, and grant sockets route only job operations.
- **Content policies.** The server, the Tauri shell and the Electron shell each set a CSP with `frame-src 'none'` and `img-src 'self' data:` (Tauri adds its asset scheme). The Tauri navigation hook allows only the packaged entry; it sees subframe navigations on macOS and Linux, so an `about:srcdoc` frame would be blocked there.
- **Persistence.** A daemon rewrites the whole `state.json` after every successfully applied op.

## Goals / Non-Goals

**Goals**
- A pane kind that shows a file or a handed-over text next to terminals, with the layout behavior of every other pane.
- The same behavior for local screens, remote screens and remote windows, and for the desktop app on Windows with its server in WSL.
- No new security surface beyond reading files the user could already `cat` in a terminal of that server; no frame, no script from a document, no change to the content policies.
- `splitty open <file>` for people and agents, without a confirmation.
- A base that `review-pane` (annotations) and later quick editing can build on.

**Non-Goals**
- Annotations, editing, diffs, renderer plugins, diagram formats, PDF.
- A file browser or a project tree.
- Viewing URLs or dev servers.

## Decisions

### D1 A viewer pane is a pane with a viewer record and no session
A pane of the role `viewer` carries `viewer: {source, originPaneId?, revision}`. `source` is `{kind: "file", path}` or `{kind: "text", title, text}`. The pane's `title` is the file name or the text's title; its `cwd` is the file's directory or, for a text, the op's `cwd`. The window generates IDs of the form `viewer-<16 hex>`.

*Alternatives:* a pane table entry with a separate document table on the screen (more bookkeeping for `move_pane_to_screen` and `close_pane`, no gain); one generic "content pane" with a URL (reopens what SR-01 removed).

### D2 Documents render in the pane without a frame (provisional, tbd)
Markdown is converted by markdown-it and HTML is taken as it is; both are sanitized by DOMPurify and inserted into a shadow root inside the pane. The sanitizer keeps no `script`, `iframe`, `frame`, `object`, `embed`, `form`, `input`, `button`, `base`, `meta`, `link`, event handler attributes, `javascript:`/`data:` link targets, `srcset`, or image sources other than `data:image/…` URLs (a dropped image shows its alt text). The style sheets of an HTML document are parsed by the CSSOM into a constructed sheet of the shadow root, and rules and declarations are removed from the parsed sheet when they load something (`url()` other than `data:image/…` and in-document `#…` references, `image-set()` and its kin, `@import`, `@font-face`) or reach outside the shadow root (`:host`, `::slotted`, `::part`); `html`, `:root` and `body` selectors are rewritten to the pane's wrappers. A declaration is checked by the text the engine will use: a value holding `var()` or another function substituted where the value is used keeps its text as written, so its escapes are resolved before the check; a shorthand holding one reads as empty longhands, so it is checked by its own serialized text, and a longhand whose value the CSSOM does not show goes. `cursor` keeps only keywords, since the system draws a cursor image around the pointer, beyond the pane. Attributes that move focus or editing into the document (`tabindex`, `accesskey`, `autofocus`, `contenteditable`, `popover`) are removed, task list boxes are inert, and DOMPurify's DOM clobbering protection is off so that headings keep their IDs, since inside a shadow root IDs and names reach neither `document` nor `window`. Links lose their `href`, which the pane keeps as `data-splitty-href`. The host element sets `contain: layout paint` and `overflow: auto`, so nothing the document positions, fixed elements included, reaches outside the pane. Clicks on links, and Enter on a focused link, are handled by the pane (middle and right clicks do nothing): `http:`/`https:` through the existing external opener (system browser), a relative link opens that file in a viewer pane (D11), `#…` scrolls within the document, anything else does nothing. The page's CSP (`script-src 'self'`, no `'unsafe-inline'` for scripts) stays a second line of defense against an inline script or handler that the sanitizer missed.

*Alternatives:*
- **A sandboxed `srcdoc` iframe** without `allow-scripts` or `allow-same-origin`: needs a deliberate change of `frame-src` in three content policies, the Tauri navigation hook blocks such a frame on macOS and Linux, and an opaque frame keeps `review-pane` from marking quoted text in the rendered view.
- **A separate, credential-free origin** for rendered content: a second listener per server and per desktop shell; large for what a viewer needs.
- **Markdown only, HTML as source**: HTML reports of agents and tools are a main use.

### D3 Text and code are shown in CodeMirror 6 (provisional, tbd)
A read-only CodeMirror 6 view with line numbers, search, wrapping on demand, and a language chosen by file name through `@codemirror/language-data`, whose languages load on demand. The editor component is the base for annotations (decorations, gutter markers) and later quick edit.

*Alternatives:* Monaco (several MB, web workers that need `worker-src`/`blob:` in three policies, theming apart from Splitty's tokens); a plain `<pre>` with a highlighter (no virtualization for large files, no editing later).

### D4 Documents are pulled over the connection in chunks (provisional, tbd)
The window reads a document with `viewer.read` and the server answers with `viewer.chunk` (at most 1 MiB of data as base64), `viewer.unchanged` or `viewer.failed`. A read starts at offset 0; every later message carries the next offset and the `version` the first answer gave. The server does not keep state between chunks: for a `file` document it validates the path with the editor request's rules, opens it with `O_NONBLOCK|O_NOCTTY` (a FIFO never blocks), takes the version from the opened file's metadata (`f-<size>-<mtime ns>-<inode>`), and fails with `changed` when that differs from the version a later chunk names. A `text` document is read the same way from the pane record (version `t-<size>-<hash>`). Limits: files up to 8 MiB, texts up to 256 KiB, at most 4 reads of one connection in flight (`busy` beyond, and also when the connection's output budget has no room for the chunk). Reads run off the connection's message loop, so a slow file system does not hold up terminal input. `viewer.read` joins `SESSION_OPERATIONS` and `docs/native-operations.json`.

Wire shapes (also in `docs/protocol.ts`):

```ts
// client → server
{ type: "viewer.read", requestId: string, paneId: string, offset: number,
  version?: string,      // required when offset > 0
  ifVersion?: string }   // offset 0 only: answer viewer.unchanged when equal
// server → client
{ type: "viewer.chunk", requestId, offset, size, version, data /* base64 */, eof: boolean }
{ type: "viewer.unchanged", requestId, version }
{ type: "viewer.failed", requestId,
  code: "unknown_pane" | "not_found" | "not_a_file" | "too_large" | "unreadable" | "changed" | "busy" | "invalid",
  error: string }
```

The pane ID alone names the document: pane IDs are unique in the workspace, and a pane that moved to another screen keeps its ID.

*Alternatives:* the document inside `mux.screen_snapshot` (snapshots go to every other client after every nudge, and scratchpad saves nudge while the user types); the server pushing all chunks at once (several viewers loading together could fill a connection's 32 MiB delivery budget and disconnect it); an HTTP route (not available on native connections and remote windows without a new catalog entry, and a second authorization path).

### D5 Shown documents stay current by polling their version (provisional, tbd)
While a viewer pane is shown and its window is visible, the window sends `viewer.read` at offset 0 with `ifVersion` every 2 seconds; the server answers with a `stat` and `viewer.unchanged`. A changed version is read in full and replaces the content, keeping the scroll position (the first visible line in source view, the scroll fraction in rendered view). A higher `revision` in a snapshot (D6) triggers a read at once.

*Alternatives:* a server-side file watcher (inotify is unreliable on WSL's `/mnt/` drives and network file systems, and would need per-connection subscriptions); no live update (an agent's plan or report would go stale while it writes).

### D6 `open_viewer` places a pane next to an anchor and reuses a file's pane only for the CLI
```ts
{ type: "open_viewer",
  paneId?: string,                                  // anchor
  placement?: "right" | "left" | "above" | "below" | "tab",   // default "right"
  newPaneId?: string,
  source: { kind: "file", path: string } | { kind: "text", title: string, text: string },
  cwd?: string }                                    // a text's directory
```
- A directional placement puts the pane next to an anchor of the root layout exactly as `split_pane` does (axis from the direction, before the anchor for `left`/`above`); `tab` appends it to the anchor's slot, in the root layout or a floating entry, and makes it active. A floating entry holds no splits, so any placement next to a floating pane becomes a tab there; `splitty open` from an agent in a floating pane thus works without `--tab`. Without `newPaneId`, a further document of a terminal goes as a tab into the group of the first viewer opened from that terminal (provisional, tbd): an agent that shows a plan, then a report, then a file keeps one viewer group beside its terminal instead of narrowing the terminal with each split. A window does the same for its own viewers by sending `tab` next to that viewer. Without an anchor the pane becomes the only pane of an empty root layout, or the right half of a new horizontal split around the root layout.
- `originPaneId`: the anchor when it is a terminal pane, the anchor's own origin when it is a viewer pane, none otherwise. `review-pane` sends annotations there by default.
- Without `newPaneId` (the CLI), a `file` source whose path a viewer pane of the screen already shows makes that pane (the first in layout order) its slot's active pane and increases its `revision`; nothing is added. An anchor that yields an origin replaces the pane's origin, so a review goes by default to the terminal that last asked to show the file. With `newPaneId` (a window, which applies the op optimistically) the server always adds the pane, so the window's panel and the server's pane agree; the window checks for an existing pane itself.
- The op has no effects. The reducer validates only what it can see: absolute paths of at most 4,096 bytes under the rules of the editor request (no `//` prefix, backslash, dot segment, control or invisible formatting character), so that no viewer pane names a path the read refuses; a text of at most 256 KiB; a title of 1 to 200 characters without control characters. Whether the file exists is decided when it is read; the CLI checks it before it sends.
- The text of a `text` source stays in the pane record, and so in `state.json`, but `mux.screen_snapshot`, `GET /api/mux/state` and `GET /api/mux/screen/{id}` leave it out (one helper strips it, as `without_scratchpads` does for scratchpads); windows read it with `viewer.read`.

The limits are provisional (tbd): 256 KiB covers terminal selections of several thousand lines while `state.json` stays small, since it is rewritten after every op.

### D7 `splitty open <file>` shows files in Splitty (provisional, tbd)
`splitty open [path] [--with vscode|zed] [--editor] [--tab] [--screen <id>] [--pane <id>] [--session <id>]`. A regular file without `--with` or `--editor` goes to a viewer pane: the CLI canonicalizes the path, refuses a file over 8 MiB, and posts `open_viewer` without `newPaneId` to `/api/mux/op` for the caller's screen and pane (implicit targeting), placed to the right, or as a tab with `--tab`. It prints `Opened <path> in pane <id>` or, for a reused pane, `Showing <path> in pane <id>`, and exits 0 at once. A folder, `--with` and `--editor` keep the editor hand-over unchanged, with its confirmation. The control API answers `open_viewer` with `paneId` and `reused`.

A viewer needs no confirmation: it starts nothing native, nothing leaves the server's machine but to the user's own window, and any process that can reach the owner control socket can already split panes and type into terminals. This changes what `splitty open <file>` did before (the editor hand-over); `--editor` restores it per call.

*Alternatives:* keep the editor as default and add `--in splitty` (the viewer is the common case for agents, and the reserved flag is long); a setting for the default (another place to look up what a command does).

### D8 Agents use the owner socket; grant sockets get no viewer access (provisional, tbd)
Agents in ordinary panes reach the owner control socket (`SPLITTY_SOCK`) and open viewers like they split panes. A restricted grant socket stays limited to job operations: opening a viewer would widen its surface, and an agent started with `agent exec` can still print a path for the user. The CLI keeps refusing `open` under `SPLITTY_AGENT_SOCK`.

### D9 A terminal selection becomes a text document
"Open Selection in Viewer" in the terminal pane's context menu, shown only with a selection and on a server that announced `viewer-v1`, creates a viewer pane to the right of the terminal with a `text` source. The text is the emulator's selection with trailing whitespace removed from every line, titled `Selection from <pane title>`, and `cwd` is the terminal's last known directory. A selection over 256 KiB is refused with a notice before anything is sent. Terminal output is plain text by then, so Markdown that an agent printed is shown as text.

### D10 Splits and tabs from a viewer start in its directory
`split_pane` and `new_tab` with a viewer pane as source start a terminal in the op's `cwd`, else the viewer's directory, with the op's shell profile or the default one. This uses the existing fallback of `create_terminal_pane` to the source pane's `cwd`; only the spec text changes.

### D11 Links between documents open further viewer panes (provisional, tbd)
A relative link in a rendered document is resolved against the document's directory; if a viewer pane of the screen shows that file, it becomes the active tab, else the window opens one as a tab next to the current viewer (`open_viewer` with `newPaneId`, `placement: "tab"`, the current viewer as anchor, so the origin carries over). Links of `text` documents are not followed. A link may name any file the server's user can read, by an absolute path or by `../` past the document's folder, as a plan names the files it changes; it opens without a confirmation, since it only shows that file in this window. While the pointer is over a link, or a link has the focus, the viewer bar shows its target in place of the document's path: the resolved path, the URL, or that the pane refuses it.

### D12 Presentation by name and content
- `.md`, `.markdown`, `.mdown`: rendered Markdown (GFM tables, task lists, fenced code highlighted with the same CodeMirror languages), switchable to source.
- `.html`, `.htm`: rendered HTML on a light page background (most HTML assumes one), switchable to source.
- PNG, JPEG, GIF, WebP and SVG by extension and magic bytes: an `<img>` from a `data:` URL (an SVG image runs no script).
- Other files whose first 8 KiB hold no NUL byte: source with highlighting by file name, decoded as UTF-8 with replacement characters.
- Other files: a notice with the size.
- Rendered mode up to 2 MiB; larger Markdown or HTML opens as source.
- `text` documents: source, plain text.

### D13 Older windows and older servers
A window offers viewer actions only on servers that announced `viewer-v1`, and builds a pane of the role `viewer` as a viewer pane only from such a server's snapshot. A pane of the role `viewer` whose record is missing or malformed is shown as a viewer that says so; a window never starts a terminal session for a viewer pane. An older window against a newer server rebuilds a viewer pane as a terminal pane and starts a shell for it outside the model (known limitation of mixed versions; `splitty connect` installs the window's own version on a host, and the browser UI is always the server's own).

## Behavior details

Settled during the implementation:
- A read answered `busy`, or cut off by a lost connection while the pane already shows content, keeps that content and is tried again; chunks that do not fit together restart the read up to three times before the pane shows `changed`.
- A viewer pane whose `revision` rose in a snapshot comes to the front of its group, also over a tab the window had in front (provisional, tbd).
- The viewer's context menu adds Wrap/Unwrap Lines and Copy Path to D9's entries.
- Markdown front matter shows as a YAML code block; a link with an empty path does nothing.
- A selection's lone surrogates become U+FFFD before it is sent, and a working directory that is not absolute is left out.
- Saving a template leaves viewer panes out and drops a group that holds only viewers (provisional, tbd).
- A new viewer pane the server made its slot's active pane is shown in front as well.
- A snapshot from another client that only changes records and shown tabs, or adds or removes viewer panes, is applied to the window's layout in place, so terminals keep the keyboard and viewers their view; any other snapshot rebuilds the screen, and viewer panes keep their mode, wrapping and reading position through the rebuild.

## Frontend structure

- `panes/viewer_pane.ts`: the dockview content renderer, registered as `viewer` in `layout/dockview_manager.ts`. It finds its binding with `bindingForApi`, loads the viewer view with `import()` at its first mount (CodeMirror, markdown-it and DOMPurify stay out of the window's startup bundle), mounts it, wires DOM focus as `TerminalPaneRenderer` does (dockview's `focus()` only marks the panel active), re-mounts when moved to a popout, and stops polling on dispose.
- `lib/viewer/documents.ts`: one reader per server connection (held by `ServerConnection`): chunked reads by `requestId`, version and `ifVersion`, timeouts, at most one read per pane at a time, the polling timer while shown.
- `lib/viewer/presentation.ts`: kind detection (D12), pure and tested.
- `lib/viewer/markdown.ts`: markdown-it setup (GFM tables, task lists, heading IDs, source line attributes for `review-pane`); `lib/viewer/sanitize.ts`: the DOMPurify configuration and the style sheet cleaning (D2); `lib/viewer/render.ts`: the shadow root and link handling; `lib/viewer/links.ts`: link resolution with the path rule of `lib/viewer/limits.ts`; `lib/viewer/syntax.ts`: languages on demand for the source view and code blocks.
- `panes/viewer_view.ts`: what the pane shows (document, notices, the bar with the path, a link's target and the switch); `panes/viewer_view_state.ts`: mode, wrapping and reading position per pane, kept through a rebuild; `lib/viewer/panel.ts`: viewer panels in dockview; `lib/viewer/syntax_colors.ts`: the syntax colors of the theme, without CodeMirror.
- `lib/viewer/source_view.ts`: the CodeMirror setup (theme from Splitty's tokens, a keymap that leaves Splitty's shortcuts alone).
- `layout/screen_snapshot.ts`: the component by role (D13), viewer params from the record, and the in-place application of snapshots that only change records, shown tabs, or viewer and job panes; `lib/mux/incoming.ts`: the `viewer` record parsed with bounds; `lib/panes.ts`: `addViewerPane` with `open_viewer`.
- `lib/context_menus.ts`: "Open Selection in Viewer" (D9); a viewer pane's own menu (Copy, Reload, Show Source/Rendered, Split, Close).
- `lib/template_extract.ts` skips viewer panes; `components/chrome/Drawer.svelte` shows them with their title; `layout/tab_renderer.ts` falls back to the viewer's title.

## Risks / Trade-offs

- **Sanitizer bypass in the main document.** A DOMPurify bypass would place markup in Splitty's document. The CSP without inline scripts keeps it from running code, and the shadow root and containment keep it inside the pane. The configuration and its tests are part of this change, and DOMPurify is pinned and updated like other dependencies.
- **CSS that loads.** The CSSOM shows a substituted value (`var()`, `if()`) as written or, for a shorthand, not at all, and jsdom does not reproduce that. The cleaning checks such values by their text and drops what it cannot see; the browser check of the change renders hostile style sheets in Chromium and logs every request.
- **Text a document hides.** Text laid out but made invisible by the document's CSS is part of a selection in the rendered view, so a copy can carry text the user did not see. The source view shows everything.
- **Rendering on the window's thread.** A heavy document of up to 2 MiB is parsed and styled on the window's main thread and can stall it for a moment.
- **Polling cost.** One `stat` every 2 seconds per shown viewer pane, only while the window is visible; negligible next to terminal output.
- **Bundle size.** CodeMirror, markdown-it and DOMPurify add about 480 KB of script, loaded with the first viewer pane rather than at startup; languages load on demand as separate chunks. The server embeds the chunks and both desktop shells serve them from the build output. After a server upgrade a window of the old version can fail to load them; the pane then asks for a reload.
- **Behavior change of `splitty open`.** Users who relied on `splitty open <file>` opening VS Code now need `--editor` or `--with`. The command's output says where the file went.
- **Mixed versions.** See D13.

## Migration

None. `state.json` gains optional fields that older daemons ignore when reading; a downgraded daemon keeps the viewer panes as panes without a session.

## Architecture impact

No new crate, top-level module or dependency between components: the reducer work stays in `splitty_core::mux`, the read handler in `splitty_server::client_session`, the CLI in `splitty::cli`. The description of the `web` container in `architecture/landscape.c4` names the viewer panes and their inert rendering. `docs/architecture.md` gains a section "Viewer Panes" (the record, `open_viewer`, reading over the connection, the text kept out of snapshots, inert rendering), and "Two Protocol Paths" lists the `viewer.*` messages.

## Provisional decisions

Taken on 2026-11-06 without the maintainer's confirmation, open to feedback; each has a bullet in `openspec/open-decisions.md`:
- **D2**: rendering in a shadow root after DOMPurify, no frame.
- **D3**: CodeMirror 6 for source.
- **D4**: limits (8 MiB files, 256 KiB texts, 1 MiB chunks, 4 reads in flight) and the pull protocol.
- **D5**: polling every 2 seconds instead of a file watcher.
- **D6**: reuse only without `newPaneId`; the 256 KiB text limit; further documents of a terminal as tabs of its first viewer's group.
- **D7**: files go to the viewer by default, `--editor` for the old behavior.
- **D8**: no viewer access through grant sockets.
- **D11**: relative links open further viewer tabs.
