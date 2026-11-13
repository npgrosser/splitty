## MODIFIED Requirements

### Requirement: Operation Rejection
The server SHALL reject a submitted operation with status 400 and a JSON body `{"ok": false, "error": string}` when the op is not a recognized operation or is malformed (error text prefixed `invalid op:`), or when the workspace rules reject it (for example an unknown screen or pane). A rejected op MUST leave the workspace state unchanged and MUST NOT notify any client connection.

#### Scenario: Unknown op type
- **WHEN** a client posts an op with `"type": "explode_pane"`
- **THEN** the server responds with status 400, `ok` false and an error starting with `invalid op:`

#### Scenario: Op on an unknown screen
- **WHEN** a client posts a valid `close_pane` op with a `screenId` that does not exist
- **THEN** the server responds with status 400, `ok` false and an error naming the missing screen
- **AND** the workspace state is unchanged

### Requirement: Operation Effects and Client Notification
An operation applied through `POST /api/mux/op` SHALL take full effect, including starting and terminating the terminal sessions it implies (see `terminal-sessions`). After an accepted op every client connection, native or through the browser gateway, SHALL receive one `mux.pane_spawned` per successfully spawned pane and then one `mux.nudge` with reason `cli_op` and the actual affected screen ID, in that order (see `mux-protocol`); after an accepted `move_pane_to_screen`, one such nudge for the source and one for the target. A failed spawn SHALL emit no successful spawn event for that session; any retained failed job state SHALL still be synchronized. State validation rejection SHALL notify nobody.

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
Through the browser gateway or the equivalent authorized native operation (`command.send`), `POST /api/command` SHALL forward a client command (`pane.activate`, `pane.copy`, `pane.paste`, `pane.clear`, `pane.close` or `pane.split`; shapes in `docs/protocol.ts`) as a `command` event to every client connection of the server, native or through the browser gateway, and respond with status 200 and `{"ok": true, "sent": n}`, `n` being the number of connections it was dispatched to. The server MUST NOT apply the command to the workspace itself and MUST NOT queue it for clients that connect later.

#### Scenario: One UI client connected
- **WHEN** one window is connected and a client posts `{"type": "pane.split", "direction": "right"}` to `/api/command`
- **THEN** that window's connection receives a `command` event carrying the command
- **AND** the server responds with `{"ok": true, "sent": 1}`

#### Scenario: No UI client connected
- **WHEN** no client connection is open and a client posts a valid command to `/api/command`
- **THEN** the server responds with status 200 and `{"ok": true, "sent": 0}`
- **AND** the command is discarded

#### Scenario: Unknown command type
- **WHEN** a client posts `{"type": "pane.explode"}` to `/api/command`
- **THEN** the server responds with a 4xx status and forwards nothing

### Requirement: Build and Runtime Information
Through the browser gateway or the equivalent authorized native operation, `GET /api/build` SHALL return `{"version", "commit", "buildTime"}` for the running binary, with `commit` and `buildTime` set to `unknown` when the build did not record them, and, when the server runs inside WSL, `wslDistro` with the name of its distribution. The server SHALL offer no `/api/stats` and no `/api/server/settings` route and no `stats.get`, `serverSettings.get` or `serverSettings.update` operation.

#### Scenario: Build info
- **WHEN** an authorized client sends `GET /api/build`
- **THEN** the server responds with status 200 and the server version, commit and build time

#### Scenario: Build info inside WSL
- **WHEN** an authorized client sends `GET /api/build` to a server running in the WSL distribution `Ubuntu`
- **THEN** the response also carries `"wslDistro": "Ubuntu"`, and outside WSL it carries no `wslDistro`

#### Scenario: Removed runtime routes
- **WHEN** an authorized client sends `GET /api/stats` or the native operation `stats.get`
- **THEN** the route is not found and the operation is unknown

### Requirement: Template Extraction Endpoint
Authenticated windows of earlier releases SHALL be able to extract a template with `POST /api/templates/extract/screen` or native `template.extract`, using the request and response in docs/protocol.ts. Missing screens SHALL return 404 and screens with no tiled terminals SHALL return 400. This endpoint SHALL NOT be exposed on owner control or restricted agent sockets. It is deprecated: windows of this release extract over the Screen's own connection (see `mux-protocol`, Template Extraction Over Session Connections), and the endpoint stays for windows of earlier releases, which use it for local Screens.

#### Scenario: Transport parity
- **WHEN** the same extraction is requested over authenticated HTTP and native API
- **THEN** both return equivalent templates or equivalent errors without changing the screen

#### Scenario: Window of an earlier release
- **WHEN** a native window of the previous release extracts a local Screen with `template.extract`
- **THEN** the server answers with the same template `mux.extract_template` gives for that Screen
