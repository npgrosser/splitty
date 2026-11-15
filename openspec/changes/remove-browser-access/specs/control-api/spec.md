## ADDED Requirements

### Requirement: HTTP Form Of Native Operations
Requirements that name an operation by an HTTP method and an `/api/...` path name its HTTP form in the operation catalog, which the native operation of the same catalog entry shares (validation, status and error object). No listener SHALL serve that form: the owner control socket routes only the paths of Control Socket Route Set (see `access-control`) and answers 404 for every other one, and every other operation is reached only as its native operation over an authorized client connection, within the role that connection has (see `client-connections`, Enumerated Native Requests).

#### Scenario: Native operation with an HTTP name
- **WHEN** a native main client requests `host.list`, which the catalog also names `GET /api/remote/hosts`
- **THEN** it receives the known hosts

#### Scenario: The same path on the control socket
- **WHEN** a pane process sends `GET /api/remote/hosts` or `GET /api/home/settings` over the owner control socket
- **THEN** the server answers 404 and changes nothing

## MODIFIED Requirements

### Requirement: Two Control Transports
The server SHALL serve the control API on its local Unix control socket, and the equivalent authorized native operations of a client connection (see `client-connections`) SHALL behave identically to the control socket's endpoints, for malformed requests as for valid ones. The server SHALL serve the control API on no TCP listener. The control socket accepts requests without any credential because access to it is restricted to the owning user. Where the socket lives, which endpoints it exposes and how it is protected are defined in `access-control` (Owner-Only Control Socket, Control Socket Route Set); a missing optional control socket SHALL not start an HTTP listener. Equivalent authorized native UI operations SHALL use the same service behavior without exposing additional control or grant routes.

#### Scenario: Same endpoint on both transports
- **WHEN** a client sends `GET /api/mux/status` over the control socket and a native client requests the equivalent status operation
- **THEN** both receive the same response content

#### Scenario: Control socket needs no credential
- **WHEN** a process of the owning user sends `GET /api/mux/status` over the control socket without any token or cookie
- **THEN** the server responds with status 200

#### Scenario: Malformed request on both transports
- **WHEN** a client posts a body to `/api/mux/op` over the control socket that is no JSON, or JSON without the operation's shape, or sends `GET /api/mux/neighbor` with a query argument the endpoint does not define, and a native client sends the equivalent malformed operation
- **THEN** the socket answers with a 4xx status and a JSON error object, the native client receives the equivalent error, and the workspace state is unchanged

#### Scenario: No TCP control listener
- **WHEN** a daemon or an embedded service runs
- **THEN** it listens on no TCP port for control requests, and `GET /api/mux/status` sent to `127.0.0.1:9800` reaches no Splitty server

### Requirement: Shared Workspace State
The control API SHALL read and modify the same workspace state that native clients use (see `mux-protocol` and `workspace-model`), so that a change made through either path is visible to the other immediately after it is applied.

#### Scenario: Op visible to a later query
- **WHEN** a client renames a screen through `POST /api/mux/op` and then sends `GET /api/mux/screens`
- **THEN** the screen list shows the new name

#### Scenario: UI change visible to the CLI
- **WHEN** a native client splits a pane and a CLI then sends `GET /api/mux/screen/{id}` for that screen
- **THEN** the returned screen contains the new pane

### Requirement: Server Status
`GET /api/mux/status` SHALL return a JSON object with `serverMode` (`daemon` or `embedded`, see `server-modes`), `version`, `screens` (number of screens), `sessions` (number of sessions across all screens), `activeScreen` (the active screen id, or `null` when there is none) and `listenAddr` (always `null`, since the service has no TCP listener), plus `instance` and `transportVersion`.

#### Scenario: Status of a daemon
- **WHEN** a client sends `GET /api/mux/status` to a server running as a daemon with two screens and three sessions
- **THEN** the response has `serverMode` `daemon`, `screens` 2 and `sessions` 3

#### Scenario: No listener address
- **WHEN** a client sends `GET /api/mux/status` to a daemon or an embedded service
- **THEN** `listenAddr` is `null`

### Requirement: Operation Effects and Client Notification
An operation applied through `POST /api/mux/op` SHALL take full effect, including starting and terminating the terminal sessions it implies (see `terminal-sessions`). After an accepted op every native client connection SHALL receive one `mux.pane_spawned` per successfully spawned pane and then one `mux.nudge` with reason `cli_op` and the actual affected screen ID, in that order (see `mux-protocol`); after an accepted `move_pane_to_screen` or `move_tab_group_to_screen`, one such nudge for the source and one for the target. A failed spawn SHALL emit no successful spawn event for that session; any retained failed job state SHALL still be synchronized. State validation rejection SHALL notify nobody.

#### Scenario: Split from the CLI reaches the UI
- **WHEN** a CLI splits a pane through `POST /api/mux/op` while a UI client is connected
- **THEN** a session is started for the new pane
- **AND** the UI client receives `mux.pane_spawned` for the new pane and then `mux.nudge` with reason `cli_op`

#### Scenario: Op without spawned sessions
- **WHEN** a CLI renames a screen through `POST /api/mux/op`
- **THEN** every client connection receives a `mux.nudge` with reason `cli_op` and no `mux.pane_spawned`

#### Scenario: Move from the CLI
- **WHEN** a pane moves from `s1` to `s2` through `POST /api/mux/op` while a UI client is connected
- **THEN** the UI client receives `mux.nudge` with reason `cli_op` for `s1` and for `s2`

### Requirement: Forwarding Commands to UI Clients
The authorized native operation `command.send` SHALL forward a client command (`pane.activate`, `pane.copy`, `pane.paste`, `pane.clear`, `pane.close` or `pane.split`; shapes in `docs/protocol.ts`) as a `command` event to every client connection of the server, and answer with `{"ok": true, "sent": n}`, `n` being the number of connections it was dispatched to. The server MUST NOT apply the command to the workspace itself and MUST NOT queue it for clients that connect later.

#### Scenario: One UI client connected
- **WHEN** one window is connected and a client sends `command.send` with `{"type": "pane.split", "direction": "right"}`
- **THEN** that window's connection receives a `command` event carrying the command
- **AND** the operation answers `{"ok": true, "sent": 1}`

#### Scenario: No UI client connected
- **WHEN** no client connection is open and a client sends a valid command with `command.send`
- **THEN** the operation answers `{"ok": true, "sent": 0}`
- **AND** the command is discarded

#### Scenario: Unknown command type
- **WHEN** a client sends `command.send` with `{"type": "pane.explode"}`
- **THEN** the operation answers with a client error and forwards nothing

### Requirement: Build and Runtime Information
The authorized native operation `build.get` SHALL return `{"version", "commit", "buildTime"}` for the running binary, with `commit` and `buildTime` set to `unknown` when the build did not record them, and, when the server runs inside WSL, `wslDistro` with the name of its distribution. The server SHALL offer no `/api/stats` and no `/api/server/settings` route and no `stats.get`, `serverSettings.get` or `serverSettings.update` operation.

#### Scenario: Build info
- **WHEN** an authorized native client requests `build.get`
- **THEN** the server answers with the server version, commit and build time

#### Scenario: Build info inside WSL
- **WHEN** an authorized native client requests `build.get` from a server running in the WSL distribution `Ubuntu`
- **THEN** the response also carries `"wslDistro": "Ubuntu"`, and outside WSL it carries no `wslDistro`

#### Scenario: Removed runtime routes
- **WHEN** a client sends `GET /api/stats` or `GET /api/build` over the control socket, or the native operation `stats.get`
- **THEN** the route is not found and the operation is unknown

## REMOVED Requirements

### Requirement: Authorization on the HTTP Listener
**Reason**: The HTTP listener was the browser gateway's, which is removed. The control socket's protection is `access-control`, Owner-Only Control Socket and Control Socket Route Set; native operations are admitted by `client-connections`, Enumerated Native Requests.

### Requirement: Health Check
**Reason**: `/health` was an unauthenticated probe of the browser gateway's HTTP listener. With no listener there is nothing to probe; a daemon's liveness is `splitty daemon status`, and a native client learns it from transport liveness (provisional, tbd).

**Migration**: Use `splitty daemon status` (exit status 0 while the daemon runs) instead of `curl http://127.0.0.1:<port>/health`.
