# Proposal

> Draft migrated from the roadmap on 2026-10-09. Not planned yet. Specs and tasks are written when this change is next. Depends on: Milestone 1 (done); builds on the baseline capabilities `workspace-model`, `cli`, `control-api` and `access-control`. The earlier concept note docs/splitty-open.md (pane by default, overlay on request, renderers chosen per format) still applies.

## Why

Splitty shows terminals, but agents and developers constantly produce files that a terminal cannot show well: Markdown, diagrams, images, PDFs, components. Today they have to leave Splitty to look at them. `splitty open <file>` renders such a file next to the terminal it came from, without Splitty building its own renderers: a fully web-based, sandboxed document viewer can be embedded identically in the browser UI and the desktop apps.

## What Changes

- **`splitty open <file>`**: a new CLI command that opens a file in Splitty. By default it opens a new viewer pane (closeable, splittable and resizable like a terminal pane). `--overlay` shows the file over the current pane as a quick preview, closed with `Escape` or `q` (UI conventions, since a viewer is not a shell). `--format <name>` names the format when the extension is ambiguous (for example `--format mermaid flow.txt`).
- **Viewer panes**: a pane kind next to terminal panes that shows rendered file content instead of a terminal session and receives the file content plus a format hint when it is opened. The workspace model already has a `viewer` pane role, but no operation creates one and the UI deliberately has no non-terminal pane component; this change introduces both.
- **Renderer lookup by manifest**: an installed renderer declares a name, an entry page and the extensions it handles. `splitty open` picks the renderer that handles the file's extension (or `--format`) and opens a viewer pane with that renderer's entry and the file. That lookup is the whole plugin logic in this change.
- **One bundled renderer**: without one, `splitty open` has nothing to render, so integrating a first renderer is part of this change, not a follow-up. It should cover Markdown, HTML, diagram formats (Mermaid, Graphviz and similar), images and PDF; which viewer it is gets decided when the change is next.

Out of scope:
- The general plugin architecture sketched in the roadmap doc as an outlook: plugin tiers beyond static rendering (interactive web plugins with a postMessage API, system plugins with a backend process, full-stack plugins), a `@splitty/plugin-api` package and a permission/capability manifest for plugins. These become their own change once a second renderer needs them.
- Rendering URLs, web apps or dev-server previews in a pane. Web-app panes and the `/proxy/` route were removed for security (SR-01/SR-02, commit 5912c28) and stay removed; links keep opening in the system browser.
- Native (non-web) renderers.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- `file-viewer`: viewer panes, how a file's content and format reach the pane, renderer selection by extension or explicit format, and overlay presentation.

### Modified Capabilities
- `workspace-model`: an operation that creates a `viewer` pane without a terminal session (as a new pane or in a floating entry for the overlay), and what closing, moving, floating and persisting mean for a pane that has no session.
- `cli`: the `open` command with `--overlay` and `--format`, using the existing discovery and implicit pane/screen targeting.
- `control-api` / `mux-protocol`: how the CLI hands the file to the server and how a client receives viewer content (an op on `/api/mux/op` with the content, or a new endpoint), and how other clients learn about the new pane.
- `access-control`: deliberate MODIFIED of "Frontend Security Headers" (the CSP has `frame-src 'none'`, which forbids any iframe) and possibly of "Control Socket Route Set" if opening needs an endpoint outside the current set. See Impact.

## Impact

Affected areas: the CLI, the mux operations and their validation, the frontend layout (a viewer component in dockview, which today throws on unknown components by design), the CSP and frame policy, and bundling of the renderer into the frontend build and the desktop apps.

Security constraints from the baseline that the design must respect:
- An iframe running scripts on Splitty's own origin is exactly what SR-01 removed: embedded content could reach the DOM, the session and the terminal API. A viewer must run in an opaque-origin sandbox (no `allow-same-origin`) or on a separate origin, and relaxing `frame-src` must be scoped to that.
- File content is untrusted. Rendering JSX/TSX, MDX, Svelte and HTML means executing it, so the renderer's sandboxing becomes part of Splitty's security boundary.
- The desktop shells refuse to render other origins and route every navigation natively (SR-27); a viewer on a separate origin must fit that policy.

Open questions for the design:
- **Bundling**: the renderer as an npm dependency, a git submodule, or a separate build embedded as an asset?
- **Content transfer**: the UI may run on another machine than the server (`splitty connect`) or on Windows while the server runs in WSL, so the file has to travel through the protocol rather than as a path or `file://` URL. Does the CLI read the file or the server, and what are the size limits?
- **File watching**: live update when the file changes (hot reload for diagrams and docs)?
- **Persistence**: a daemon persists the workspace; should a restored viewer pane re-read its file, keep the last content, or be dropped?
- **Overlay**: reuse floating entries for `--overlay`, or a separate transient overlay that is not part of the layout?
- **Editor integration**: triggering `splitty open` from vim/neovim (`:!splitty open %` already works from inside a pane once the command exists; a native integration is a separate question).


## Carried-over design notes

- 2026-10-18, `cli-editor-open`: `splitty open [path] [--with vscode|zed]` now provides confirmed external editor handoff; integrated Bash/Zsh also expose `splitty-open`. This viewer change must integrate with that command rather than introduce a conflicting default. An explicit `--in splitty` destination is reserved for the later viewer design, not implemented by the external editor change. The viewer sandbox requirements above remain applicable.
