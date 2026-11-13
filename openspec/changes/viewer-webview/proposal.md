# Proposal

> Draft created on 2026-11-06. Not planned yet. Specs, design and tasks are written when this change is next. Depends on: `splitty-open` (viewer panes), `remote-ports` and `remote-ports-auto` (forwards of host ports announced as `localhost`). Related: `viewer-sandboxed-html` (scripted documents without network), `viewer-renderers`.

## Why

Agents in Splitty build web applications and start their development servers in a pane. To look at the result, the user switches to the system browser, finds the port and keeps two windows side by side; on a host the port has to be forwarded first. A pane that shows the running application next to the terminal, and follows the agent's restarts, closes that gap.

Splitty had such panes once and removed them (SR-01): the page was proxied through `/proxy/{port}` on Splitty's own origin, so the application's scripts could reach the window's DOM, its session and the terminal API. This change brings the pane back only on the opposite principle: the page never runs on Splitty's origin or with the shell's native authority. It is loaded from its own origin, and its only connection to Splitty is the pane it is shown in.

## What Changes

- **Web view panes**: `splitty open http://localhost:<port>[/path]` from a pane, and "Open in Pane" next to "Open in browser" for detected and forwarded ports, show the page in a pane. Only `http://localhost:<port>` addresses (with `127.0.0.1`, `0.0.0.0` and `[::1]` rewritten to `localhost`), not arbitrary internet addresses (provisional).
- **The page's own origin**: the page is loaded directly from `http://localhost:<port>`, which is a different site from the window's `http://127.0.0.1:<port>` and from the desktop shells' packaged origins. No route on the gateway serves or proxies it; `/proxy/{port}` and the other removed web-content routes keep answering 404. The window's guards already refuse anything the page sends to Splitty: its `Origin` is not on the allowlist, its requests are `cross-site` under the Fetch Metadata rule, and the `SameSite=Strict` session cookie of `127.0.0.1` is not sent to `localhost`.
- **Hosts**: a remote pane's port is shown through a forward, opened the way a click on a `localhost` link opens one (`remote-ports-auto`), and the pane follows the forward when it is rebuilt.
- **In the window**: a frame without `allow-top-navigation` and without popups (provisional), so a page cannot navigate the window or open windows of its own; the window's `frame-src` admits `http://localhost:*` instead of `'none'`. The desktop shells keep child frames without any native grant, and their navigation guards admit child-frame navigations only in frames that a web view pane created.
- **The pane's controls**: reload, back and forward, the address, "Open in browser". A page that refuses to be framed (`X-Frame-Options`, `frame-ancestors`) shows a notice with "Open in browser".
- **The record**: the pane is persisted with its address like a viewer pane, and comes back after a restart without reloading anything but the page.

Out of scope:
- Internet addresses and pages that need a login kept in a cookie. In a frame the page is a third-party context: cookies with `SameSite=Lax`, the browsers' default, are not sent there.
- A proxy of any kind on Splitty's origin, including one that rewrites the page.
- Annotations on web view panes: the window cannot read a page on another origin. A screenshot to annotate is a candidate for later.
- Developer tools for the page and device emulation.

## Capabilities

To be decided when the change is next. Likely candidates:

### New Capabilities
- `webview-panes` (or as part of `viewer-panes`): the pane, its addresses, the controls, the behavior on refusal and restart.

### Modified Capabilities
- `access-control`: "Frontend Security Headers" (`frame-src`), the desktop shells' navigation guards and child frames, the rule that no gateway route serves or proxies web content, the `localhost` rule for frames as for forwards.
- `cli`: `splitty open <url>`.
- `remote-screens`: "Open in Pane" for ports, and forwards opened for a pane.
- `workspace-model` and `mux-protocol`: the pane's record and its opening operation.

## Impact

- Frontend: the web view pane, its controls, the notices, "Open in Pane" in the port lists.
- Server: the pane's record, `splitty open <url>`, no new route.
- Desktop shells: `frame-src` in `crates/splitty-tauri/tauri.conf.json` and `frontend-electron/src/assets.ts`; child-frame navigation in `crates/splitty-tauri/src/navigation.rs` and `frontend-electron/src/main.ts`; whether each engine allows `http:` child frames in a page of a packaged scheme.
- Tests: a hostile page in a frame in a real browser (the window's DOM, the API with and without the cookie, top navigation, popups, the shell's native commands in each desktop shell); the existing tests that pin SR-01 and SR-02 change only in the `frame-src` value.
- Open questions:
  - A frame in every shell, or a native web view of its own in the desktop shells (a separate web contents without preload or IPC, its own storage, layered over the pane). A native web view is a first-party context with its own process, so logins work, but it covers the window's menus and dialogs and needs hiding while they are open.
  - Whether a window opened as `http://localhost:<port>` instead of `127.0.0.1` refuses web view panes, since the page would then share the host name, and the cookie, with the window.
  - Whether a later change admits internet addresses, and under which policy.

## Carried-over design notes

- Security findings (`docs/security/findings.md`): SR-01 removed `/proxy/{port}`, the web-app panes and `/api/webapps`, because pages on Splitty's origin reached the DOM, the session and the terminal API; the paths answer 404 with a valid credential (test `webview_routes_are_absent_even_with_a_valid_token`). SR-02 restricted the external opener to `http:` and `https:` and added `frame-src 'none'` as the regression guard (test `app_document_carries_a_strict_csp`). SR-27 added the navigation guards of the desktop shells and keeps non-packaged origins in the system browser.
- 2026-10-12 and 2026-10-13, `remote-ports` and `remote-ports-auto` (`access-control`): forwards are bound to `127.0.0.1` and announced only as `http://localhost:<port>`, so that the browser does not send them Splitty's session cookie, which belongs to the host `127.0.0.1`. A web view pane relies on the same split.
- 2026-11-06, `viewer-renderers` (draft): URLs, web applications and development servers were left out there, pointing to this change.
- 2026-11-13, `remove-electron-shell`: the Electron shell (`frontend-electron/`) is removed. What this change says about Electron, its main process, preload or packaged assets no longer applies; the desktop shell around the web frontend is Tauri, and the native app (`crates/splitty-gpui`) is the default desktop app.
