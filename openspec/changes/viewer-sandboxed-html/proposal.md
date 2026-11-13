# Proposal

> Draft created on 2026-11-06. Not planned yet. Specs, design and tasks are written when this change is next. Depends on: `splitty-open` (viewer panes and the inert rendering). Related: `viewer-renderers` (renderers that do not run a document's code), `viewer-webview` (pages served by a development server), `review-pane` and `review-threads` (annotations on the rendered view).

## Why

Viewer panes render HTML inert: scripts, event handlers, frames and network loads are stripped, so an HTML document shows its markup and styles and nothing more. Many HTML files that agents and tools produce only work with their scripts: charts and reports with inline JavaScript, notebooks exported to HTML, interactive mock-ups of a user interface, single-file prototypes. Today they need the system browser, outside the terminal and the review flow.

Running a document's scripts inside Splitty's window is safe only if the scripts cannot reach what the window can: its DOM, its session cookie, the server's API, the terminals and the native shell. The inert rendering reaches that by removing everything active. This change reaches it for running scripts by isolation: the document runs in a frame with an origin of its own that has no network access and one narrow message channel to its pane.

## What Changes

- **Run scripts in a viewer pane**: an HTML document can be shown with its scripts running, in a frame inside the pane. The inert rendering stays the default; running scripts is the user's choice for a pane, one click away and remembered with the pane (provisional).
- **An opaque origin, enforced twice**: the frame is sandboxed with scripts allowed and without `allow-same-origin`, and the frame document's own response carries a `sandbox` directive in its Content Security Policy, so the document gets an opaque origin even when its address is opened directly in a tab.
- **No network**: the frame document's policy allows no connections (`connect-src 'none'`) and loads scripts, styles, images, fonts and media only from inline code, `data:` and `blob:`. The document is handed to the frame by the pane, never fetched by the frame. Single-file documents work; documents that load sibling files are limited to what the window inlines (see the open questions).
- **No way out of the frame**: no top-level navigation, popups, forms, modal dialogs, downloads, pointer lock or presentation, and no permissions (clipboard, camera, microphone, geolocation). A navigation of the frame itself, which browsers cannot forbid, ends the document: the pane replaces the frame with a notice.
- **One message channel**: the pane and the frame talk only through `postMessage` with a fixed set of messages (the document in; size, selection, scroll position and link clicks out). The pane accepts messages only from its own frame, validates and bounds them, and never evaluates what it receives. Links still open through the external opener or as files in a viewer pane, as in the inert rendering.
- **Annotations on the rendered view**: selections in the frame reach the pane as quotes, so the annotation flow of `review-pane` and `review-threads` works on scripted documents too. The frame receives only the passages to mark, never the annotations' bodies or replies.
- **Security headers and the desktop shells**: `frame-src` of the window's policy (`access-control`, Frontend Security Headers, and the policies of both desktop shells) admits exactly the frame document's address instead of `'none'`; the navigation guards of the desktop shells (SR-27) admit that address for child frames and nothing else; child frames keep receiving no native grant.

Out of scope:
- Pages served by a development server or any other URL: `viewer-webview`.
- Documents fetching data over the network or reading other files at run time (`fetch`, WebSocket, `XMLHttpRequest`).
- Running scripts by default for every HTML document, and a trust list of folders or hosts.
- Other formats that execute code (JSX, MDX, notebooks with live kernels). They can reuse the frame once this change exists.

## Capabilities

To be decided when the change is next. Likely candidates:

### Modified Capabilities
- `access-control`: "Rendered Documents Are Inert" gains the sandboxed exception with its rules; "Frontend Security Headers" changes `frame-src` from `'none'` to the frame document's address; the frame document's own headers; the desktop shells' navigation guards and child frames.
- `viewer-panes`: the scripted presentation of HTML, the per-pane choice, what happens when the frame navigates away.
- `viewer-annotations`: annotations made in the scripted view.

## Impact

- Frontend: a frame host component in the viewer, the message channel, the notice on navigation, the per-pane choice in the view state.
- Server: a static frame document with its own headers (a path of the gateway, served without a credential like the application shell's assets).
- Desktop shells: the frame document in each shell's packaged assets with its own headers, or a scheme of its own (which also gives the frame a site of its own); `frame-src` in `crates/splitty-tauri/tauri.conf.json` and `frontend-electron/src/assets.ts`; the navigation guards in `crates/splitty-tauri/src/navigation.rs` and `frontend-electron/src/main.ts`.
- Tests: the existing CSP test (`app_document_carries_a_strict_csp`) changes with the policy; new tests for the frame document's headers, a hostile document in a real browser (cookie, API, parent DOM, top navigation, popup, self-navigation), and the shells' child-frame rules.
- Open questions:
  - Where the frame document comes from in each shell: a gateway path with a `sandbox` policy in the browser; a scheme of its own in Tauri and Electron.
  - Whether the window inlines relative scripts, styles, images and fonts next to the document (bounded count and size, restricted to asset types, so that a document cannot pull an arbitrary sibling file into its reach), or single-file documents are the limit.
  - Whether scripts run per pane only, or also per document path.
  - How a busy script is contained where the engine runs the frame in the window's process (WebKit; Chromium isolates sandboxed frames in a process of their own only in recent versions): a watchdog through the message channel, or accepting that closing the pane is the remedy.

## Carried-over design notes

- 2026-11-06, `splitty-open`: the inert rendering (sanitizer, shadow root, no inline scripts, CSS sanitizer, `data:` images only) is specified in `access-control` as "Rendered Documents Are Inert", including that rendering needs no change to Frontend Security Headers. This change is the first to change that, and only for the sandboxed frame.
- 2026-11-06, `viewer-renderers` (draft): renderers that run a document's code were left out there because they need an opaque-origin sandbox, which `frame-src 'none'` and the desktop shells' guards forbid today.
- Security findings (`docs/security/findings.md`): SR-01 removed same-origin web content (`/proxy/{port}`), which could reach the window's DOM, session and terminal API; SR-02 keeps `frame-src 'none'` as its regression guard; SR-27 added native navigation guards and keeps non-packaged origins out of the shells. None of these may be weakened for anything but the opaque-origin frame document, and the tests that pin them (`webview_routes_are_absent_even_with_a_valid_token`, `app_document_carries_a_strict_csp`) keep guarding the rest.
- Guards that already refuse a request from the frame (`access-control`): the Origin allowlist (an opaque origin sends `Origin: null`), the Fetch Metadata same-origin rule (such a request is `cross-site`) and the `SameSite=Strict` session cookie. The frame's `connect-src 'none'` is the first line, these are the second.
- Remaining risks to state in the design: a document can show anything inside its pane, including a fake prompt, and can send what it shows or what the user types into it to an outside address by navigating its own frame before the pane notices. Its own content is what its author put there; the pane hands it nothing else.
- 2026-11-13, `remove-electron-shell`: the Electron shell (`frontend-electron/`) is removed. What this change says about Electron, its main process, preload or packaged assets no longer applies; the desktop shell around the web frontend is Tauri, and the native app (`crates/splitty-gpui`) is the default desktop app.
