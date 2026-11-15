## Why

`remove-browser-access` and `remove-tauri-shell` removed the web client, its shells and the browser gateway. The server still carries protocol messages, operations, event fields, capabilities, settings and theme keys whose only reader or sender was that web client, plus code and test leftovers that describe a browser, a gateway or a WebSocket as present. Nothing that remains uses them: the released native clients from 0.3.0 on, the CLI and `splitty attach`, agent grant clients, and hubs and hosts of other versions. They widen what the server accepts and what a reader of the specs has to understand, and some spec requirements still describe the system by what the web client did.

This change removes that surface after checking each item against every remaining client, and rewords the spec text that described the system through the web client where the statement still holds for the desktop app. It follows `remove-browser-access`; no roadmap milestone carries it.

## What Changes

- **BREAKING** (wire, old web senders only): the session messages `create` and `close` are no longer accepted; panes get their sessions through workspace operations and `mux.pane_spawned`, and a session ends with its pane.
- **BREAKING** (wire): the workspace operation `reorder_screens` goes; `move_screen` places screens.
- **BREAKING** (wire): the native operations `shellProfiles.update` (`PUT /api/shell-profiles`) (provisional, tbd), `host.list` (`GET /api/remote/hosts`), `hostPreferences.get` (`GET /api/remote/host-preferences`), `hostOrder.get` (`GET /api/remote/host-order`), `catalog.get` and `catalog.update` (`GET`/`PUT /api/remote/catalog`) leave the operation catalog. The writes and the directory that current windows use stay: `host.forget`, `hostPreferences.update`, `hostOrder.update`, `hub.catalog.observe`, `remote.directory.get`.
- **BREAKING** (wire): the events `created` and `title` are no longer sent; `attached` loses `homeDir`; a notification `notice` loses `kind`; `mux.welcome` loses `clientId`, `screens` and `serverMode` and carries its `capabilities`.
- **BREAKING** (wire): the welcome capabilities `scratchpad-v1` and `serverDerivedPaneState` are no longer announced; the behavior they announced stays.
- `GET /api/mux/status` no longer carries `listenAddr`, which has been `null` since the TCP listener went.
- **BREAKING** (transport): the native transport hello no longer has the host-open intent `openHost`; a hello carrying it is refused on every carrier. Host streams open through the owner helper's open command, as the desktop app does.
- **BREAKING** (settings): the settings `terminal.renderer`, `terminal.ligatures`, `terminal.windowPaddingBalance`, the group `terminal.xterm.*`, the groups `tabs.*`, `window.*` and `experimental.*`, and `drawer.resizable` go. A `config.toml` holding them loads without error, they are ignored and dropped on the next write; an update naming them is refused as an unknown setting. The tolerance for the earlier removed `terminal.linkTarget` and `terminal.webviewMode` in updates goes too (provisional, tbd).
- **BREAKING** (themes): the theme sections `[effects]` and the layout tokens `dnd-overlay-scope` and `dnd-overlay-target` are no longer read or returned; theme files that hold them still load. The theme answer keeps an empty `layout` object for released native clients that require it (provisional, tbd).
- Code: the CLI's single-variant transport type becomes a socket path, the test harness drops `tokio-tungstenite`, the test-only host-open fixture goes, unused connect-supervisor errors in the SSH tunnel crate go, and comments and test names that describe a browser, a gateway, a WebSocket or the web frontend as present are reworded or removed.
- Specs: requirements that described the system through the web client (`create`, `close`, `title`, the welcome's screen list, the xterm clipboard client, "where the browser pastes", "in the web and in the native client") are reworded for the desktop app or removed.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `terminal-sessions`: sessions start only for panes; `create`, `close`, `created` and `title` go; shell, command, directory and environment rules are stated for pane spawns; `attached` loses `homeDir`; the default shell profile is set in `config.toml`.
- `mux-protocol`: the welcome carries capabilities only; client identity, the server-derived pane state capability and the welcome's screen list go; windows build their workspace from the workspace catalog; notices lose `kind`.
- `workspace-model`: `reorder_screens` leaves the operation list and the screen order rules.
- `session-scratchpad`: `scratchpad-v1` goes.
- `viewer-annotations`: handing a review to a scratchpad no longer depends on `scratchpad-v1`.
- `server-modes`: the mode is reported by `GET /api/mux/status` only.
- `control-api`: `listenAddr` leaves the status; the example operation of the HTTP form changes.
- `remote-screens`: the known hosts, host preferences, host order and catalog read routes and the catalog write route go.
- `client-connections`: the transport hello has no host-open intent.
- `access-control`: the terminal clipboard policy is stated for the desktop app's terminals.
- `settings`: the removed setting groups and fields; update validation without the xterm scroll fields.
- `display-zoom`: zoom settings without web-based shells.
- `program-status`: the busy reference points at the snapshot pane state instead of the removed `title` metadata requirement.

## Impact

- Code: `crates/splitty-core` (native operation catalog, protocol messages and capabilities, session dispatcher, mux operations, settings, theme types and bundled themes, notices), `crates/splitty-server` (session handler, mux handler, service operations and dispatch, daemon endpoint, native client fixtures, CLI discovery, control tests), `crates/splitty-transport` (hello and negotiation), `crates/splitty-remote` (tunnel errors and comments), `crates/splitty-client-core` (comments only).
- Wire: `docs/protocol.ts` loses the entries of the removed messages, fields and operations.
- Version skew: no released native client, CLI, hub or host sends or reads a removed item (evidence per item in `design.md`). A window of a released version keeps working with a host of this version, and this version keeps working with older hosts and hubs.
- Persisted data: `config.toml` and theme files keep loading; `remote-catalog.json`, `remote-host-preferences.json`, `remote-host-order.json`, templates and the workspace are unchanged.
- Docs: `docs/architecture.md`, `docs/issues/openspec-baseline-findings.md`, `openspec/open-decisions.md`, the descriptions in `architecture/server.c4`.

## Out of scope

- Anything a remaining client uses, also when the web client used it too: `theme.get`/`theme.list`, `template.extract`/`template.save`, `remoteScreens.update`, `command.send`, `mux.neighbor`, the session job messages, `attached`'s semantic fields, `bell`, `remote.progress` tolerance, `managed-jobs-v1` and `hub-auto-forward-v1`, `appearance.borderRadius`, `drawer.showTips`, `nativeTabs.*`, the UI and terminal theme tokens.
- Renames (for example `SPLITTY_BROWSER_ASKPASS`), refactors beyond the removal, and new features.
- The hidden `serve`, `connect` and `issue-bootstrap` stubs (`remove-browser-access`, RBA-9).
- `docs/protocol.ts` as a whole: it keeps its readers (specs, config and crate comments refer to it); only the entries of removed items change.
- The exposure registry's decoding of older gateway claims (RBA-12) and opening URLs in the system browser.
