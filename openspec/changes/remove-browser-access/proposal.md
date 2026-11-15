## Why

Since `native-default-desktop` the native app is the desktop client of the releases, and it reaches the local service through the private native client endpoint (no TCP, no token) while the CLI reaches it through the owner control socket and the agent grant sockets. The Svelte web client and the browser gateway that serves it are no longer released, yet they keep a loopback TCP listener, a process token, cookies, bootstrap codes, WebSocket tickets and a second client implementation alive, each of which the security invariants, the tests and every protocol change still have to carry. This change removes everything that exists only so that a browser can reach Splitty.

It is the second step of removing the web parts: `remove-tauri-shell` (the legacy desktop shell around the web frontend) lands before it, and a later follow-up removes the catalog operations, mux and session messages, capabilities and state that only the web client used but that are not part of the browser gateway itself. There is no roadmap milestone of its own; it continues the cutover of 120 ([Native UI mit GPUI](../native-ui-gpui/proposal.md)).

## What Changes

- **BREAKING** `splitty serve` (plain and `--daemon`), `splitty connect` and `splitty issue-bootstrap` are removed without replacement. Persistent local work stays `splitty daemon start`, the desktop app opens hosts itself, and terminal access without the app stays `splitty attach`, also over SSH (`ssh -t host splitty attach`). The removed commands answer with a short usage error that names these ways for the transition (provisional, tbd).
- **BREAKING** The global `--addr` and `--token` options and the CLI's TCP transport are removed: control commands and `splitty attach` reach a server only through the owner control socket, a grant socket, the private native endpoint or a keeper. `SPLITTY_ADDR` no longer means anything.
- The process token is gone: nothing reads or writes `auth-token` any more; an existing file is left alone (provisional, tbd).
- The browser gateway is removed from the server: the loopback TCP listener and its accept loop, the Host, Origin and Fetch Metadata guards, browser sessions with their `splitty-session-<instance>` cookies, bootstrap codes, WebSocket tickets, `/health`, `/ws`, `/ws/remote` with its `remote.progress` translation, the embedded frontend with its security headers, the `web-assets` feature and the browser frame translation. The owner control socket keeps its HTTP framing and its route set; the router, its job routes and the shared request translation stay and move under the control socket's module.
- `splitty-remote` loses what only `connect` and the gateway used: the gateway listener of the exposure registry, the session URL, the `connect` event stream, the `--forward` list parser and the generic connect supervisor where nothing else uses it. Port forwards, reverse sharing, the exposure registry for shared ports, install, handshake, the SSH supervisor parts the hub uses and the askpass broker stay.
- The Svelte web client in `frontend/` is deleted with its tests and tooling; builds, CI, CodeQL's JavaScript analysis, Dependabot's npm entry for it, the release scripts, `Makefile` targets and test scripts drop it. The architecture check under `architecture/` keeps its own pnpm package.
- The smoke tests that started `splitty serve` or `connect` are deleted where they test the gateway and moved to the daemon, the owner backend (`desktop-backend --owner`) or the native endpoint where they test the service, the hub or remote hosts.
- Specs, the architecture model, `docs/architecture.md`, `docs/networking.md`, `docs/protocol.ts`, `SECURITY.md`, `docs/security/`, `README.md`, `CLAUDE.md`, `openspec/config.yaml`, the roadmap and the drafts built on the web client stop describing browser access.

Wire compatibility: nothing that a released native client (0.3.0 and later) sends or relies on changes. A host daemon of this version keeps serving such a client through `remote-bridge`, and this version's client keeps reaching older host daemons as before.

Out of scope:

- Catalog operations, mux and session messages, capabilities and server state that only the web client requested but that are not gateway code (for example theme answers, client-assignment or layout-shadow details). The design lists the candidates found on the way under "Follow-up inventory"; the follow-up change removes them.
- The Tauri shell, its CI and its specs (`remove-tauri-shell`).
- A wire reference derived from the Rust types; `docs/protocol.ts` stays the reference, without browser-only messages.
- Releasing; the next release notes the breaking CLI change.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `access-control`: the listener, guard, credential, cookie, ticket, header and token requirements lose their subject and are removed with reasons; the no-TCP-listener rule is restated on its own; the control socket, grant, keeper, native endpoint, forward and reverse-share rules stay and lose their browser clauses.
- `cli`: `serve`, `connect`, `issue-bootstrap`, the token options and the TCP target are removed (the removed commands answer with a usage error for the transition); discovery, attach, startup output, the scoped-command guard and the command surface are narrowed.
- `server-modes`: embedded mode is no longer reached through `serve`; the browser adapter lifetime and the gateway's port handling go.
- `control-api`: one control transport (the control socket) plus the equivalent native operations; `/health` and the HTTP listener's authorization go.
- `mux-protocol`: the `/ws` endpoint, WebSocket mapping and browser liveness, reconnect and layout-reporting clauses go.
- `client-connections`: browser clients and gateways leave the equivalence, lifetime, negotiation and admission rules.
- `remote-screens`: `/ws/remote`, its tickets and `remote.progress` translation go; host streams are native only, and the hub's host preparation is written out instead of referring to `connect`.
- `remote-hosts`: installation, payload, preparation and SSH-question rules name the hub instead of `connect` and lose the web client.
- `desktop-remote`: the editor handoffs lose their browser dispatch path.
- `terminal-sessions`, `settings`, `program-status`, `display-zoom`, `templates`, `viewer-annotations`, `workspace-model`, `session-keepers`: requirements and scenarios that name the web window, the browser client, the WebSocket or `serve` are narrowed to the native client.

## Impact

- Code: `crates/splitty-server` (`src/http/` except the control router, `src/embed.rs`, `src/main.rs`, `src/cli/`, `src/hub/`, tests), `crates/splitty-core/src/auth/`, `crates/splitty-remote/src/tunnel/`, `frontend/` (deleted).
- CLI: breaking for `serve`, `connect`, `issue-bootstrap`, `--addr`, `--token`; `daemon start --addr`, `daemon restart --addr` and `serve`'s flags disappear with them.
- Dependencies: `rust-embed`, `mime_guess` and the browser-only server parts of `hyper`, `hyper-util`, `tokio-tungstenite` and `reqwest` features go where nothing else uses them (the design lists them).
- Build and release: no frontend build, no pnpm in CI except for `architecture/`, no `frontend/dist` in the release.
- Docs and planning text listed in the tasks.
