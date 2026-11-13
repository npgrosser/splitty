# control-api Specification

## Purpose
The HTTP control API lets the `splitty` CLI and other automation inspect and drive a running server without a WebSocket connection: query the workspace, submit layout operations, send input to terminal sessions and forward commands to connected UI clients. It is served on the server's loopback HTTP listener and, in a reduced form, on a per-process local control socket for processes running inside panes.

## Requirements

### Requirement: Two Control Transports
The server SHALL serve the control API on its local Unix control socket and, when explicitly started, through a browser gateway's loopback HTTP listener, and an endpoint that is available on both transports SHALL behave identically on both, for malformed requests as for valid ones. Authorization on the HTTP listener follows `access-control`; the control socket accepts requests without any credential because access to it is restricted to the owning user. Where the socket lives, which endpoints it exposes and how it is protected are defined in `access-control` (Owner-Only Control Socket, Control Socket Route Set); a missing optional control socket SHALL not start an HTTP listener. Equivalent authorized native UI operations SHALL use the same service behavior without exposing additional control or grant routes.

#### Scenario: Same endpoint on both transports
- **WHEN** a client sends `GET /api/mux/status` over the control socket and another client sends it, authorized, over the HTTP listener
- **THEN** both receive status 200 with the same response content

#### Scenario: Control socket needs no credential
- **WHEN** a process of the owning user sends `GET /api/mux/status` over the control socket without any token or cookie
- **THEN** the server responds with status 200

#### Scenario: Malformed request on both transports
- **WHEN** a client posts a body to `/api/mux/op` over the control socket that is no JSON, or JSON without the operation's shape, or sends `GET /api/mux/neighbor` with a query argument the endpoint does not define, and another client sends the same request, authorized, over the HTTP listener
- **THEN** both receive the same 4xx status and the same JSON error object, and the workspace state is unchanged

### Requirement: Authorization on the HTTP Listener
Every control endpoint on the HTTP listener except `/health` SHALL require an authorized request as defined in `access-control`; an unauthorized request MUST be rejected before any handler runs and MUST NOT change any state.

#### Scenario: Unauthenticated operation
- **WHEN** a client sends `POST /api/mux/op` over the HTTP listener without valid credentials
- **THEN** the request is rejected as specified in `access-control`
- **AND** the workspace state is unchanged

### Requirement: Health Check
The server SHALL answer `GET /health` on the HTTP listener with status 200 and the plain-text body `ok`, without requiring credentials or an `Origin` header.

#### Scenario: Liveness probe
- **WHEN** a client without credentials sends `GET /health` with a loopback `Host` header
- **THEN** the server responds with status 200 and body `ok`

### Requirement: Shared Workspace State
The control API SHALL read and modify the same workspace state that native and WebSocket clients use (see `mux-protocol` and `workspace-model`), so that a change made through either path is visible to the other immediately after it is applied.

#### Scenario: Op visible to a later query
- **WHEN** a client renames a screen through `POST /api/mux/op` and then sends `GET /api/mux/screens`
- **THEN** the screen list shows the new name

#### Scenario: UI change visible to the CLI
- **WHEN** a WebSocket client splits a pane and a CLI then sends `GET /api/mux/screen/{id}` for that screen
- **THEN** the returned screen contains the new pane

### Requirement: Workspace and Screen Queries
`GET /api/mux/state` SHALL return the complete workspace state, and `GET /api/mux/screen/{id}` SHALL return the state of a single screen, with payload shapes as described by `WorkspaceState` and `ScreenState` in `docs/protocol.ts`, except that no session carries its `scratchpad` (see `session-scratchpad`), no viewer pane's `text` document carries its text (see `viewer-panes`), and no viewer pane carries its `annotations` (see `viewer-annotations`). A session that holds a root status record SHALL carry it as `programStatus` (see `program-status`). A request for an unknown screen id MUST answer status 404 with the JSON body `{"error": "screen not found"}`.

#### Scenario: Existing screen
- **WHEN** a client sends `GET /api/mux/screen/{id}` for an existing screen
- **THEN** the server responds with status 200 and that screen's layout, panes and sessions

#### Scenario: Unknown screen
- **WHEN** a client sends `GET /api/mux/screen/does-not-exist`
- **THEN** the server responds with status 404 and `{"error": "screen not found"}`

#### Scenario: Scratchpad omitted
- **WHEN** a session's scratchpad holds text and a client sends `GET /api/mux/state` or `GET /api/mux/screen/{id}` for its screen
- **THEN** the session appears without `scratchpad`

#### Scenario: Viewer text omitted
- **WHEN** a viewer pane shows a terminal selection and a process in a pane sends `GET /api/mux/state` over the control socket
- **THEN** the viewer pane appears with its `viewer` record but without the selected text

#### Scenario: Annotations omitted
- **WHEN** a viewer pane holds annotations and a client sends `GET /api/mux/state` or `GET /api/mux/screen/{id}` for its screen
- **THEN** the viewer pane appears with its `viewer` record but without `annotations`

#### Scenario: Program status included
- **WHEN** a session's program has reported `working` and a client sends `GET /api/mux/screen/{id}` for its screen
- **THEN** the session appears with `programStatus` holding the state `working`
- **AND** a session without a root status record appears without `programStatus`

### Requirement: Screen Summary List
`GET /api/mux/screens` SHALL return a JSON array with one entry per screen in workspace screen order, each carrying `id`, `name`, `paneCount` and `active`, where `active` is true for exactly the workspace's active screen and false for all others.

#### Scenario: Listing screens
- **WHEN** the workspace has screens A and B in that order and B is active
- **THEN** `GET /api/mux/screens` returns A then B, with `active` false for A and true for B

#### Scenario: Empty workspace
- **WHEN** the workspace has no screens
- **THEN** `GET /api/mux/screens` returns an empty array with status 200

### Requirement: Server Status
`GET /api/mux/status` SHALL return a JSON object with `serverMode` (`daemon` or `embedded`, see `server-modes`), `version`, `screens` (number of screens), `sessions` (number of sessions across all screens), `activeScreen` (the active screen id, or `null` when there is none) and `listenAddr` (null for the listener-independent service), plus `instance` and `transportVersion`; any browser-gateway address is reported by that gateway's startup, not fabricated as a target-service listener.

#### Scenario: Status of a daemon
- **WHEN** a client sends `GET /api/mux/status` to a server running as a daemon with two screens and three sessions
- **THEN** the response has `serverMode` `daemon`, `screens` 2 and `sessions` 3

### Requirement: Submitting Layout Operations
`POST /api/mux/op` SHALL accept a JSON body `{"screenId": string, "op": object}` and apply `op` to the workspace with the semantics defined in `workspace-model`. On success the server MUST respond with status 200 and `{"ok": true, "revision": n, "screenId": string, "created": array}`, where `screenId` is the actual affected screen, `n` is its post-op revision or zero if it was removed, and `created` lists the `paneId` and `sessionId` of successfully started sessions. For `open_viewer` the response SHALL also carry `paneId`, the viewer pane that shows the document, `reused`, true when that pane existed before (see `viewer-panes`, Reopening A File), and `viewerRevision`, the revision of that pane's viewer record after the operation. Managed-job operations SHALL use the same semantics, authorization and idempotency as the job endpoints. A session spawn failure MUST NOT be answered as `ok: true`. An op of type `apply_template`, `set_scratchpad`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent` MUST be refused with status 400 and leave the workspace unchanged (see `session-scratchpad` and `viewer-annotations`).

#### Scenario: Split a pane
- **WHEN** a client posts `{"screenId": "s1", "op": {"type": "split_pane", "paneId": "p1", "axis": "horizontal"}}` and the op is valid for screen `s1`
- **THEN** the server responds with status 200, `ok` true, `screenId` `s1`, its new revision, and the created pane/session mapping
- **AND** screen `s1` contains a new pane

#### Scenario: Create a screen without an id
- **WHEN** a client posts `{"screenId": "", "op": {"type": "create_screen", "name": "backend"}}`
- **THEN** the server creates a screen named `backend` with a server-generated id
- **AND** responds with status 200, `ok` true, that actual `screenId`, revision 1 and its successfully created pane/session mapping

#### Scenario: Failed session spawn
- **WHEN** an accepted pane-creating operation cannot start its session
- **THEN** the API returns a 5xx error with `code` `spawn_failed` and the affected identifiers, and never reports that session as successfully created

#### Scenario: Scratchpad refused
- **WHEN** a client posts an op of type `set_scratchpad`
- **THEN** the server responds with status 400, and no scratchpad changes

#### Scenario: Open a viewer
- **WHEN** a client posts `{"screenId": "s1", "op": {"type": "open_viewer", "paneId": "p1", "source": {"kind": "file", "path": "/home/me/app/PLAN.md"}}}`
- **THEN** the server responds with status 200, `ok` true, an empty `created`, the new pane's `paneId`, `reused` false and `viewerRevision` 1

#### Scenario: Open the same file again
- **WHEN** the same request is posted again while that viewer pane exists
- **THEN** the server responds with the same `paneId`, `reused` true and a higher `viewerRevision`, and the screen holds no new pane

#### Scenario: Annotations refused
- **WHEN** a client posts an op of type `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent`
- **THEN** the server responds with status 400, and no annotation changes

### Requirement: Operation Targeting
The server SHALL apply the op types `create_screen`, `close_screen`, `rename_screen`, `set_active_screen` and `move_pane_to_screen` as workspace operations and the existing other layout op types as screen operations on the screen named by `screenId`. `close_screen`, `rename_screen` and `set_active_screen` target `screenId`; `move_pane_to_screen` moves from `screenId`; `create_screen` ignores the envelope's `screenId`, takes the new screen's id from the op's own `screenId` field or generates one, and uses the default shell profile when the op names none. A screen operation whose `paneId` the named screen does not hold, while another screen does, SHALL be applied to the screen that holds the pane, so a shell whose pane moved keeps targeting it; a managed job's pane never moves, so for it the mismatch stays a conflicting target. Managed-job operations SHALL target their declared placement or job ID, as specified in `workspace-model`, and reject conflicting envelope targets rather than retargeting silently; an owner's tab or split placement whose screen no longer holds its pane SHALL follow the pane, an agent grant's SHALL NOT (see `access-control`).

#### Scenario: Screen op routed to the request's screen
- **WHEN** a client posts `{"screenId": "s2", "op": {"type": "close_pane", "paneId": "p7"}}`
- **THEN** the server closes pane `p7` in screen `s2`

#### Scenario: New screen with a client-chosen id
- **WHEN** a client posts `{"screenId": "", "op": {"type": "create_screen", "screenId": "screen-9"}}`
- **THEN** the workspace contains a new screen with id `screen-9` whose first session uses the default shell profile

#### Scenario: Split in a moved shell
- **WHEN** pane `p7` moved from screen `s1` to `s2` and its shell runs `splitty split`, which posts a `split_pane` of `p7` on `s1`
- **THEN** the server splits `p7` in `s2`

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

### Requirement: Spatial Neighbor Query
`GET /api/mux/neighbor?screen=&pane=&dir=` SHALL respond with status 200 and `{"paneId": string}` naming the neighbor of the given pane in direction `dir` (`left`, `right`, `up`/`above`, `down`/`below`) as resolved by `workspace-model` (Spatial Neighbor Resolution). `paneId` MUST be empty when the result is "no neighbor". A missing parameter or unknown `dir` MUST answer 400, an unknown screen 404.

#### Scenario: Neighbor to the right
- **WHEN** a screen has panes `p1` and `p2` side by side and a client sends `GET /api/mux/neighbor?screen=s1&pane=p1&dir=right`
- **THEN** the server responds with `{"paneId": "p2"}`

#### Scenario: No neighbor in that direction
- **WHEN** a client asks for the `left` neighbor of the leftmost pane
- **THEN** the server responds with status 200 and `{"paneId": ""}`

#### Scenario: Missing parameter
- **WHEN** a client sends `GET /api/mux/neighbor?screen=s1&pane=p1`
- **THEN** the server responds with status 400

### Requirement: Sending Input to a Session
`POST /api/session/{id}/send` SHALL accept a JSON body `{"data": string}` and write `data` unchanged, as UTF-8 bytes, to the input of the terminal session `{id}`, as if the user had typed it, responding with status 200 and `{"ok": true}`. An unknown session id MUST answer status 404 with `{"error": "session not found"}`, and a failed write MUST answer status 500 with an `error` message.

#### Scenario: Send a command line
- **WHEN** a client posts `{"data": "ls\n"}` to `/api/session/{id}/send` for a running session
- **THEN** the session receives the bytes `ls` followed by a newline
- **AND** the server responds with `{"ok": true}`

#### Scenario: Unknown session
- **WHEN** a client posts to `/api/session/nope/send`
- **THEN** the server responds with status 404 and `{"error": "session not found"}`

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

### Requirement: Side-Effect-Free Reads
Control endpoints using `GET` SHALL NOT modify workspace state, terminal sessions, configuration or credentials; every state-changing control endpoint uses `POST`.

#### Scenario: Neighbor query does not move focus
- **WHEN** a client sends `GET /api/mux/neighbor` for a pane
- **THEN** the active pane and the workspace revision are unchanged

### Requirement: Malformed Requests and Undefined Paths
The server SHALL answer a control request whose body is not JSON of the expected shape with a 4xx status without changing any state, and SHALL answer an authorized request for an undefined path under `/api/` with status 404 rather than with the application document.

#### Scenario: Missing body field
- **WHEN** a client posts `{"op": {"type": "close_screen"}}` to `/api/mux/op` without `screenId`
- **THEN** the server responds with a 4xx status and the workspace state is unchanged

#### Scenario: Removed endpoint
- **WHEN** an authorized client sends `GET /api/webapps`
- **THEN** the server responds with status 404

### Requirement: Managed Job Capability Discovery
The server SHALL advertise `managed-jobs-v1` in a `capabilities` array in `/api/mux/status`, `mux.welcome` and the job-list response. `GET /api/jobs` SHALL return an object with `capabilities` and `jobs`; the list SHALL be scoped before serialization. An absent capability SHALL mean managed jobs are unsupported, never permission to fall back to shell creation.

#### Scenario: Older server
- **WHEN** a client connects to a server without `managed-jobs-v1` or with no job endpoint
- **THEN** job commands report unsupported functionality and send no legacy create/send-keys workaround

### Requirement: Managed Job Endpoints
The API SHALL expose `POST /api/jobs`, `GET /api/jobs`, `GET /api/jobs/{id}`, `GET /api/jobs/{id}/logs`, and `POST /api/jobs/{id}/input`, `/stop`, `/forget`. They SHALL obey `managed-jobs` and `access-control` on the TCP listener, owner socket and grant sockets. Request/response shapes SHALL be documented in `docs/protocol.ts`. Grant identity SHALL come from the transport, not JSON.

#### Scenario: Same job through owner and scoped views
- **WHEN** the owner and a job's current grant each inspect that job
- **THEN** both see the same identifiers and lifecycle, while the grant's list excludes unrelated jobs

### Requirement: Job Launch Responses
A successful first spawn SHALL return 201 with the job; replay of that success SHALL return 200, while replay of failure SHALL retain its failure status. Validation SHALL return 400, a missing target 404, conflicting request data 409, capacity exhaustion 429 and spawn failure 500. An attempt still unresolved after 10 seconds SHALL return 504 with `launch_pending` and its IDs and remain available for inspection or retry.

#### Scenario: Slow start and retry
- **WHEN** an accepted start has not resolved within 10 seconds and the caller retries after the 504
- **THEN** the retry joins that attempt, and the timeout never authorizes a duplicate spawn

### Requirement: Job Log Responses
`GET /api/jobs/{id}/logs` SHALL accept `cursor` or `tail`, `limit`, `waitMs` and `format=text|raw`. A response SHALL carry `data`, `encoding`, `nextCursor`, `oldestCursor`, `endCursor`, `lostBytes`, `hasMore`, job `state`, `outputComplete` and `outputAvailable`. Invalid options or cursors SHALL return 400. A daemon-restored job whose process did not survive the restart SHALL report unavailable old output rather than fabricated retained logs. A job whose keeper the daemon adopted (see `session-keepers`) SHALL report the output its keeper retained, under a new output generation.

#### Scenario: Read after daemon restart
- **WHEN** an authorized caller reads the logs of a restored job whose keeper was gone, without an old cursor
- **THEN** the result reports `outputAvailable` false with no old output
- **AND** supplying the former cursor instead returns `invalid_cursor`

#### Scenario: Read an adopted job after a restart
- **WHEN** a daemon restarts while a job that printed `ready` runs, and the owner reads its logs without an old cursor
- **THEN** the result reports `outputAvailable` true and contains `ready`
- **AND** a cursor from before the restart returns `invalid_cursor`

### Requirement: Local Grant Administration Endpoints
Only the owner socket SHALL expose `POST /api/agents`, `GET /api/agents`, `GET /api/agents/{id}`, `POST /api/agents/{id}/revoke` and `POST /api/jobs/{id}/owner`. Creating a grant SHALL return its ID and socket path. Owner changes SHALL select a live grant or the user and obey `managed-jobs`. Grant sockets, native UI connections and authenticated TCP callers SHALL receive 404 for these paths; TCP requests rejected earlier by the existing guards/authentication SHALL retain those responses.

#### Scenario: Administrative transfer
- **WHEN** the owner socket receives a valid request to move a job to a new grant
- **THEN** the same process and output become available to that grant and inaccessible to the previous grant

#### Scenario: Administration attempted through TCP
- **WHEN** an authenticated TCP client requests grant creation, enumeration, revocation or job ownership changes
- **THEN** it receives 404 and no grant or job ownership changes

### Requirement: Bounded Job Input And API Errors
Job input SHALL accept only `{"data": string}` of at most 16 KiB of UTF-8 and write it unchanged after an execution-time ownership check. Invalid payloads SHALL return 400 or 413 without partial input. Job errors SHALL use JSON `error` and `code`, with allocated request/job IDs where applicable, without leaking unrelated job details. Input to a terminal-state job SHALL conflict.

#### Scenario: Input after job exit
- **WHEN** a caller sends input to its exited job
- **THEN** the API returns 409 and does not create or target a new shell

### Requirement: Job Request Bounds
Launch JSON SHALL be at most 128 KiB, with at most 256 arguments totaling 64 KiB, a name at most 200 characters and cwd at most 4096 UTF-8 bytes. IDs and argument strings SHALL reject embedded NULs. Log tails SHALL accept 0 through 10000 lines in text mode only. Invalid limits, conflicting placement fields or partial anchor pairs SHALL fail before effects or output reads.

#### Scenario: Oversized or contradictory launch
- **WHEN** a launch exceeds a bound, includes an embedded NUL or supplies a split direction with screen placement
- **THEN** it fails with a structured 4xx error without allocating a job or process

### Requirement: Editor Open Requests
`POST /api/editor/open` SHALL accept only a session ID, absolute existing path and optional `vscode|zed` editor (shapes in docs/protocol.ts), derive file/folder kind, and send `editor.request` only to exactly one capable attached client. No recipient or multiple recipients SHALL fail without delivery. The response SHALL distinguish dispatched, denied, failed, disconnected, expired and busy requests. Grant sockets SHALL not expose this endpoint.

#### Scenario: Sole attached recipient
- **WHEN** exactly one client advertising editor support is attached to the named live session
- **THEN** only that connection receives the request and only its matching response can settle it

#### Scenario: No recipient or ambiguous recipient
- **WHEN** zero or multiple capable clients are attached to the session
- **THEN** the endpoint fails without broadcasting, queueing or replaying a request

#### Scenario: Bounds and connection lifetime
- **WHEN** a client already has a pending request, requests arrive within two seconds, or 32 requests are pending globally
- **THEN** admission fails with 429
- **AND** a pending request expires within 30 seconds, and detachment or disconnect cancels it

#### Scenario: Input and reply validation
- **WHEN** input includes a target, URI, executable, invalid path/type/editor, or a reply names another connection's request
- **THEN** it cannot cause handoff or settle another request

### Requirement: File Offer Requests
`POST /api/file/offer` SHALL accept only `{"sessionId","path"}` with the ID of a live session and an absolute path that passes the validation of Editor Open Requests, open the file, refuse anything but a regular file and a file over 512 MiB, and offer it to the session's registered clients (see `terminal-sessions`, File Offers). It SHALL wait for the outcome and respond 200 `{"outcome":"saved"}` once a client has read the whole file, or with `{"code","error"}`: 404 `session_not_found`, 400 `invalid_path`, 413 `too_large`, 409 `no_client` without a recipient, 409 `declined`, 408 `expired`, 409 `disconnected`, 429 `busy` while 8 offers are pending on the server or 2 for the session, and 500 `read_failed` when the file cannot be read to its end. Grant sockets SHALL NOT expose this endpoint.

#### Scenario: Saved by the window
- **WHEN** a pane process offers a file and the window showing its session saves it
- **THEN** the request responds 200 with outcome `saved` after the last chunk was read

#### Scenario: No window
- **WHEN** a session that no registered client is attached to offers a file
- **THEN** the request responds 409 `no_client` and nothing is sent

#### Scenario: A folder
- **WHEN** the path names a directory
- **THEN** the request responds 400 `invalid_path`

### Requirement: Viewer Comment Requests
`GET /api/viewer/comments` SHALL take the query parameters `screenId` and `paneId`, the caller's screen and pane, and optionally `document`, an absolute path or a viewer pane's ID, and `all`. It SHALL respond 200 with `{"documents": [...]}`: for each viewer pane of the screen that shows the document, or for each viewer pane of the screen without `document`, its `paneId`, its source without a text's text, and the annotations an agent may read (see `viewer-annotations`, Agents Read And Reply To Handed-Over Threads), the open ones only unless `all` is `true`. Each annotation SHALL carry its number, kind, status, remark, replacement, target and the replies an agent may read, each with its author and, for an agent's reply, its pane and title; shapes are in `docs/protocol.ts`. `POST /api/viewer/comments/reply` SHALL take `{"screenId", "paneId", "document", "number", "body"}`, add `body` as an agent's reply with the caller's pane and that pane's title to the annotation with that number, and respond 200 `{"ok": true, "paneId": string, "number": n}`. Both endpoints SHALL respond 404 with `code` `not_found` for a screen, a caller pane or a document the screen does not hold and, for a reply, an unknown number or an annotation the user has not handed over; 409 `ambiguous`, naming the viewer panes, for a path that several viewer panes of the screen show; 400 `invalid` for a reply that is empty, only whitespace or over 16,384 bytes; and 409 `limit` for a reply the thread or the pane cannot take. Grant sockets SHALL NOT expose these endpoints.

#### Scenario: Comments of a screen
- **WHEN** a pane process requests `GET /api/viewer/comments?screenId=s1&paneId=p1` after the user sent a review on screen `s1`
- **THEN** the server responds 200 with the viewer panes of `s1` and their handed-over annotations

#### Scenario: A reply
- **WHEN** a pane process posts `{"screenId": "s1", "paneId": "p1", "document": "/home/me/app/PLAN.md", "number": 2, "body": "Done."}` to `/api/viewer/comments/reply`
- **THEN** the server responds 200, and annotation #2 holds the reply with author `agent` and pane `p1`

#### Scenario: Unknown number
- **WHEN** the same request names number 9, which the pane does not hold
- **THEN** the server responds 404 with `code` `not_found`, and nothing changes

### Requirement: Review Requests
`POST /api/viewer/review/wait` SHALL take `{"screenId", "paneId", "viewerPaneId", "viewerRevision"?}`, the caller's screen and pane, the viewer pane whose review it waits for and optionally the `viewerRevision` the caller's `open_viewer` was answered with, start a wait for a review of that pane (see `viewer-annotations`, Waiting For A Review), and hold the request until the wait ends. It SHALL respond 200 with `{"outcome": "approved"}` when the user approves without notes, `{"outcome": "approved", "review": string}` when the user approves with notes, or `{"outcome": "changes_requested", "review": string}` when the user requests changes; 410 with `code` `closed` when the viewer pane closes first; 404 `not_found` for a screen, caller pane or viewer pane the screen does not hold, or a pane that is not a viewer pane; 409 `busy` while another wait for the same viewer pane exists; 409 `changed`, without a wait, when the viewer record carries another revision than `viewerRevision` as the wait would start; and 429 `busy` while 16 waits exist on the server. When the caller's connection ends before an answer, the server SHALL end the wait without an outcome. Grant sockets SHALL NOT expose this endpoint.

#### Scenario: Approved
- **WHEN** a pane process waits for viewer pane `v1` and the user approves in a window without notes
- **THEN** the request responds 200 with `{"outcome": "approved"}`

#### Scenario: Changes requested
- **WHEN** a pane process waits for `v1` and the user requests changes
- **THEN** the request responds 200 with `{"outcome": "changes_requested", "review": …}` holding the window's review

#### Scenario: Second wait
- **WHEN** a second request for `v1` arrives while the first waits
- **THEN** it responds 409 with `code` `busy`, and the first keeps waiting

#### Scenario: Replaced before the wait
- **WHEN** a process shows a text in `v1`, answered with `viewerRevision` 3, another process's text replaces it before the first waits, and the first then waits with `viewerRevision` 3
- **THEN** the request responds 409 with `code` `changed`, and `v1`'s viewer record names no wait

#### Scenario: Caller gone
- **WHEN** the waiting process is killed
- **THEN** the wait ends, and `v1`'s viewer record no longer names it

### Requirement: Template Extraction Endpoint
Authenticated windows of earlier releases SHALL be able to extract a template with `POST /api/templates/extract/screen` or native `template.extract`, using the request and response in docs/protocol.ts. Missing screens SHALL return 404 and screens with no tiled terminals SHALL return 400. This endpoint SHALL NOT be exposed on owner control or restricted agent sockets. It is deprecated: windows of this release extract over the Screen's own connection (see `mux-protocol`, Template Extraction Over Session Connections), and the endpoint stays for windows of earlier releases, which use it for local Screens.

#### Scenario: Transport parity
- **WHEN** the same extraction is requested over authenticated HTTP and native API
- **THEN** both return equivalent templates or equivalent errors without changing the screen

#### Scenario: Window of an earlier release
- **WHEN** a native window of the previous release extracts a local Screen with `template.extract`
- **THEN** the server answers with the same template `mux.extract_template` gives for that Screen
