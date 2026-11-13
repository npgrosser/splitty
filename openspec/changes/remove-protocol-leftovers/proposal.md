# Proposal

## Why

The protocol and the persisted state still carry pieces that no supported peer uses: an event nobody reads, an event no server builds, operations without a caller, a pane role no operation creates, and a second way to extract a template. Each one is code, docs and tests to keep in step with the rest.

## What Changes

- The server no longer sends `osc133` events. The prompt boundaries still feed the notification engine and the program status; neither client read the event.
- The `notification` event type goes from the protocol and from both clients. No server of this or the previous release builds it.
- The operations `stats.get` (`GET /api/stats`), `serverSettings.get` and `serverSettings.update` (`/api/server/settings`) go. No client calls them.
- The pane role `webview` goes. A daemon that restores a `state.json` holding such panes removes them like closed panes; a window reads the role from an older host as `terminal`. Template panes of the type `webview` are dropped wherever a template is read (its file, a request, an older host's answer), and their `url` field with them; the filters in the server's projection and in both clients go.
- Both windows extract a template over the Screen's own connection (`mux.extract_template`), for local and host Screens alike. `template.extract` and `POST /api/templates/extract/screen` stay served for windows of earlier releases and are deprecated.
- The control API spec describes command forwarding and op notifications for every client connection instead of WebSocket clients.
- Kept: `mux.neighbor` and `command.send` (scripting API, handled by both clients); the per-connection `title` event with its raw fields, which the web client reads also from current servers (the Drawer's shell marker, close confirmations, the working directory for editors and new panes, viewer titles); the client-side replay filters, now with a comment on when they can go.

## Capabilities

### New Capabilities

### Modified Capabilities
- `control-api`: build and runtime information without `/api/stats`; command forwarding and op notifications reach every client connection; the extraction endpoint is deprecated.
- `mux-protocol`: the fallback for servers without `notification-notices-v1` no longer lists `notification`; both windows extract over the Screen's connection.
- `templates`: legacy web panes are dropped when a template is read, without a notice.
- `server-modes`: restoring a workspace removes legacy web panes.

## Impact

- `splitty-core`: `ServerEvent::SemanticBoundary` and `ServerEvent::Notification`, `PaneRole::Webview`, template `Pane::url`, the web-pane handling of `mux::templates`, the restore in `daemon::persistence`, the operation catalog (`native_api`).
- `splitty-server`: the three operations, the dispatch arms, the smoke tests that used `/api/stats` as an authenticated probe.
- `splitty-client-core` and `splitty-gpui`: template filters, `skipped_web_count`, extraction through `TemplateApiRequest::Extract`.
- `frontend/`: protocol types, the template projection and its notice, the generic `notification` in the notification list.
- Docs: `docs/protocol.ts`, `docs/native-operations.json`, `docs/osc-133-semantic-prompts.md`, `docs/architecture.md`.

## Architecture impact

none (no component or dependency changes; docs/architecture.md loses the legacy web pane pruning and names one extraction path for windows).

## Decisions

- **PL-1** (provisional, tbd): the windows converge on `mux.extract_template` rather than on `template.extract`. A host's connection carries only the session stream in both windows, and the gateway's API reaches only the serving server, so `template.extract` cannot read a host's Screen without a new channel. The server keeps answering `template.extract` for native windows of earlier releases, which use it for local Screens.
- **PL-2** (provisional, tbd): a restored `webview` pane is removed from the layout like a closed pane, and a Screen left without panes goes with it; a pane of the role `webview` that an older host still sends is read as `terminal`, keeping the host's layout intact in the window (the web client already did this). Alternative: restore it as a terminal pane, which would leave a pane without a session.
- **PL-3** (provisional, tbd): a template's legacy web panes are dropped without a notice. They have loaded nothing for several releases; a template that held only web panes can no longer be applied and says so.
