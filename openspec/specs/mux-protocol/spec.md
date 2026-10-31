# mux-protocol Specification

## Purpose
The real-time WebSocket protocol between GUI clients and the server: how a connection is established and kept alive, how messages are framed, how clients learn and resynchronize workspace state, how layout ops are submitted and their outcomes reported, and how changes and server commands reach connected clients. Payload shapes are defined in `docs/protocol.ts`; layout op semantics live in `workspace-model`, session message semantics in `terminal-sessions`.

## Requirements

### Requirement: WebSocket Endpoint
The server SHALL expose the real-time protocol on a single WebSocket endpoint at `/ws` and SHALL only complete the upgrade for requests that are authorized as defined in `access-control` (allowed origin and a single-use `ticket` query parameter).

#### Scenario: Authorized upgrade
- **WHEN** a client opens `/ws?ticket=<ticket>` from an allowed origin with a valid, unused ticket
- **THEN** the server SHALL complete the WebSocket upgrade
- **AND** the server SHALL send `mux.welcome` on the new connection

#### Scenario: Unauthorized upgrade
- **WHEN** a client opens `/ws` without a valid ticket or from a disallowed origin
- **THEN** the server SHALL reject the request with an HTTP error before upgrading
- **AND** no protocol message SHALL be sent

### Requirement: Shared Connection for Control and Terminal I/O
One WebSocket connection MUST carry both workspace control messages and terminal session messages. The server SHALL route every client text frame whose `type` starts with `mux.` to workspace control, and every other client text frame to the session protocol defined in `terminal-sessions`.

#### Scenario: Mux message routing
- **WHEN** a client sends a text frame with `type` `mux.resync`
- **THEN** the server SHALL handle it as a workspace control message

#### Scenario: Session message routing
- **WHEN** a client sends a text frame with `type` `input` on the same connection
- **THEN** the server SHALL handle it as a session message as defined in `terminal-sessions`

### Requirement: Message Encoding
Client-to-server messages SHALL be JSON objects sent as WebSocket text frames, discriminated by a string `type` field. Server-to-client events other than terminal output SHALL be JSON objects sent as text frames, discriminated by `type`; every `mux.*` event and every `command` event MUST carry a unique event `id` and a `timestamp` in Unix milliseconds.

#### Scenario: Server event envelope
- **WHEN** the server sends a `mux.nudge` event
- **THEN** the event SHALL be a JSON text frame with `type` `mux.nudge`, an `id`, and a `timestamp`

### Requirement: Binary Terminal Output Frames
The server SHALL deliver terminal output exclusively as binary WebSocket frames with the layout `[1 byte: session ID length N][N bytes: session ID][payload bytes]`. The server SHALL ignore binary frames sent by a client.

#### Scenario: Output frame layout
- **WHEN** the server delivers output bytes for session `sess-abc`
- **THEN** it SHALL send one binary frame whose first byte is 8, followed by the bytes of `sess-abc`, followed by the output bytes

#### Scenario: Client sends a binary frame
- **WHEN** a client sends a binary frame
- **THEN** the server SHALL discard it without a response and keep the connection open

### Requirement: Malformed and Unknown Messages
The server MUST NOT close a connection because of a malformed or unknown message. Text frames that are not valid JSON or lack a `type` SHALL be ignored. Unknown `mux.*` types, `mux.op`/`mux.resync` messages missing required fields, and `mux.op` messages whose `op` is not a recognized operation SHALL be ignored without a response. A non-`mux.` message with an unknown `type` SHALL be answered with an `error` event naming that type.

#### Scenario: Invalid JSON
- **WHEN** a client sends a text frame that is not valid JSON
- **THEN** the server SHALL ignore it and the connection SHALL stay open

#### Scenario: Unknown session message type
- **WHEN** a client sends `{"type": "frobnicate"}`
- **THEN** the server SHALL send an `error` event whose message names `frobnicate`

#### Scenario: Unknown mux message type
- **WHEN** a client sends `{"type": "mux.attach", "screenId": "s1"}`
- **THEN** the server SHALL ignore the message without a response

### Requirement: Welcome on Connect
Immediately after the upgrade, without waiting for any client message, the server SHALL send `mux.welcome` containing the connection's `clientId`, the list of screens in workspace order (each with `id` and optional `name`), and `serverMode` set to `embedded` or `daemon` (see `server-modes`).

#### Scenario: Welcome lists existing screens
- **WHEN** a client connects to a server whose workspace holds screens `C`, `A`, `B` in that order
- **THEN** `mux.welcome.screens` SHALL list `C`, `A`, `B` in that order

#### Scenario: Empty workspace
- **WHEN** a client connects to a server with no screens
- **THEN** `mux.welcome.screens` SHALL be an empty list

### Requirement: Per-Connection Client Identity
The server SHALL assign each connection a fresh random `clientId` of 32 hexadecimal characters. A `clientId` MUST NOT be reused for a later connection, and the protocol SHALL offer no way to resume a previous `clientId`.

#### Scenario: Reconnect gets a new identity
- **WHEN** a client disconnects and connects again
- **THEN** the second `mux.welcome` SHALL carry a `clientId` different from the first

### Requirement: Concurrent Clients Without Screen Ownership
The server SHALL accept multiple simultaneous connections and MUST NOT grant any connection exclusive access to a screen. There are no `mux.connect`, `mux.attach`, `mux.detach`, or `mux.busy` messages; every connected client can read and modify every screen.

#### Scenario: Second client connects
- **WHEN** a second client connects while a first client is connected
- **THEN** both clients SHALL receive their own `mux.welcome` listing all screens
- **AND** ops from either client SHALL be applied to the shared workspace

### Requirement: Screen Snapshots on Resync
On `mux.resync` with a `screenId`, the server SHALL send `mux.screen_snapshot` with the complete current state of that screen, including its `revision`, to the requesting client only. A resync for an unknown screen SHALL produce no response.

#### Scenario: Resync existing screen
- **WHEN** a client sends `mux.resync` for screen `s1`
- **THEN** the server SHALL send that client a `mux.screen_snapshot` whose `screen.id` is `s1`

#### Scenario: Resync unknown screen
- **WHEN** a client sends `mux.resync` for a screen that does not exist
- **THEN** the server SHALL send no response

### Requirement: Client Resynchronization
A client MUST treat the server's workspace state as authoritative after connecting: on `mux.welcome` it SHALL create any listed screen it does not have locally and SHALL send `mux.resync` for every listed screen. A client SHALL build every screen listed in `mux.welcome` from its `mux.screen_snapshot`, also when it has prepared the screen locally before the snapshot arrives, and MUST NOT send `create_screen` for such a screen in its place. (Re-creating a screen that later ran empty stays allowed, see `workspace-model`.) Before the first `mux.welcome` of a connection a client MUST NOT send `create_screen`. It SHALL create an initial screen of its own only when the welcome lists no screens, and SHALL show only the listed screens when the welcome lists any. On a `mux.nudge` that carries a `screenId`, a client SHALL send `mux.resync` for that screen.

#### Scenario: Reconnect restores state
- **WHEN** a client reconnects and receives `mux.welcome` listing screens `s1` and `s2`
- **THEN** the client SHALL send `mux.resync` for `s1` and for `s2`

#### Scenario: Nudge triggers resync
- **WHEN** a client receives `mux.nudge` with `screenId` `s1`
- **THEN** the client SHALL send `mux.resync` for `s1`

#### Scenario: Page reload against an existing workspace
- **WHEN** a client starts while the server's workspace holds screen `s1` whose panes run sessions
- **THEN** the client SHALL send `mux.resync` for `s1` and no `create_screen`
- **AND** its view of `s1` SHALL show those panes connected to their existing sessions

#### Scenario: First start against an empty workspace
- **WHEN** a client starts and `mux.welcome` lists no screens
- **THEN** the client SHALL create one screen with `create_screen`

### Requirement: Submitting Layout Ops
A client SHALL submit layout changes as `mux.op` messages carrying a `screenId` and an `op` object whose `type` selects the operation; op semantics and validation are defined in `workspace-model`. The server SHALL process the messages of one connection strictly in arrival order and SHALL send no acknowledgement for an accepted op other than the `mux.pane_spawned` events it causes.

#### Scenario: Accepted op without new sessions
- **WHEN** a client sends a valid `mux.op` that creates no session
- **THEN** the server SHALL apply it and send no response to that client

#### Scenario: Ops applied in order
- **WHEN** a client sends two `mux.op` messages in sequence
- **THEN** the server SHALL apply them in the order sent

### Requirement: Pane Spawn Notification
When an op submitted over the WebSocket successfully starts a session for a pane, the server SHALL send `mux.pane_spawned` with that `paneId` and the new `sessionId` to the originating client, and SHALL enqueue it before processing that client's next message. A failed spawn SHALL send `mux.op_failed` with the pane and actual screen identifiers instead, and SHALL never advertise the absent session as spawned. A client receiving `mux.pane_spawned` SHALL connect the pane to the given session through the session protocol (see `terminal-sessions`) instead of creating a session itself.

#### Scenario: Split creates a session
- **WHEN** a client sends a `mux.op` that creates pane `p2` with a new session
- **THEN** the client SHALL receive `mux.pane_spawned` with `paneId` `p2` and the server-assigned `sessionId`

#### Scenario: Job program cannot start
- **WHEN** a job op is accepted but execution fails
- **THEN** the originating client receives failure rather than a successful spawn event, and synchronized job state reports `failed`

### Requirement: Rejected Op Reporting
When the server rejects a recognized op that names a pane to create (`newPaneId`) because it fails validation (see `workspace-model`), it SHALL send `mux.op_failed` to the originating client only, carrying the op `type`, the `screenId`, that `paneId`, and an error message. When it rejects an `arrange_panes` op, it SHALL send `mux.op_failed` to the originating client only, carrying the op `type`, the `screenId` and an error message.

#### Scenario: Rejected pane-creating op
- **WHEN** a client sends a `mux.op` with `newPaneId` `p9` that the server rejects
- **THEN** the client SHALL receive `mux.op_failed` with `paneId` `p9` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected arrangement
- **WHEN** a client sends `arrange_panes` on screen `s1` that misses one of its panes
- **THEN** the client SHALL receive `mux.op_failed` with `op` `arrange_panes`, `screenId` `s1` and a non-empty `error`
- **AND** no other client SHALL receive it

### Requirement: Change Propagation to Other Clients
When an op submitted over the WebSocket spawns or terminates sessions, the server SHALL send every other connected client the op's `mux.pane_spawned` events followed by `mux.nudge` with reason `mux_op` and the op's `screenId`; the originating client SHALL NOT receive that nudge. When a `move_pane_to_screen` op is applied, the server SHALL send every other connected client `mux.nudge` with reason `mux_op` for the source screen and for the target screen, and none to the originating client. When a `set_scratchpad` or an `arrange_panes` op is applied, the server SHALL send every other connected client `mux.nudge` with reason `mux_op` for its screen, and none to the originating client.

#### Scenario: Session-changing op from a client
- **WHEN** client A sends a `mux.op` on screen `s1` that spawns a session while client B is connected
- **THEN** client B SHALL receive `mux.pane_spawned` and then `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive the nudge

#### Scenario: Pane moved by a client
- **WHEN** client A moves a pane from screen `s1` to screen `s2` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` for `s1` and for `s2`
- **AND** client A SHALL receive neither

#### Scenario: Scratchpad saved by a client
- **WHEN** client A sends `set_scratchpad` on screen `s1` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive it

#### Scenario: Panes arranged by a client
- **WHEN** client A sends an accepted `arrange_panes` on screen `s1` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive it

### Requirement: Propagation of HTTP-Applied Ops
When an op is applied through the HTTP API (see `control-api`), the server SHALL send every connected client `mux.nudge` with reason `cli_op` and the actual affected screen's ID, preceded by `mux.pane_spawned` for each session the op successfully spawned. A newly generated screen ID SHALL never be replaced by the empty request envelope's ID. Managed-job lifecycle changes SHALL synchronize their affected pane state without requiring a UI output subscription.

#### Scenario: Op applied via HTTP
- **WHEN** an op on screen `s1` is applied through the HTTP API
- **THEN** every connected client SHALL receive `mux.nudge` with reason `cli_op` and `screenId` `s1`

#### Scenario: Job in a new screen
- **WHEN** a job creates a server-generated screen through the API
- **THEN** clients receive its actual screen ID and can fetch its snapshot without reconnecting

### Requirement: Spawn-Before-Nudge Ordering
For any single op, the server MUST deliver every `mux.pane_spawned` event for that op to a client before the `mux.nudge` for the same op, so that a resync never discards a pane that is still waiting for its session ID.

#### Scenario: Ordered delivery
- **WHEN** an op spawns sessions for panes `p2` and `p3` and is propagated to a client
- **THEN** that client SHALL receive `mux.pane_spawned` for `p2` and `p3` before the `mux.nudge` for that op

### Requirement: Server Commands
The server SHALL deliver a server-initiated `command` event to every connected client, carrying a nested `command` object whose `type` is one of `pane.activate`, `pane.copy`, `pane.paste`, `pane.clear`, `pane.close`, or `pane.split` (with `direction`), and an optional `paneId`. A client SHALL apply a command only to panes of the screens it shows from the server whose connection delivered the command: to the named pane, or, when `paneId` is absent, to the active pane of its active screen if that screen belongs to that server. Otherwise it SHALL ignore the command. A command SHALL NOT change which screen is active. How commands are triggered is defined in `control-api`.

#### Scenario: Split command without pane
- **WHEN** a client receives `command` with `{"type": "pane.split", "direction": "right"}` and its active screen belongs to the server that sent it
- **THEN** the client SHALL split its active pane to the right

#### Scenario: Broadcast to all clients
- **WHEN** a command is issued while two clients are connected
- **THEN** both clients SHALL receive the `command` event

#### Scenario: Pane of another server
- **WHEN** a window shows screens of two servers and one of them sends `pane.close` naming a pane id that only exists on a screen of the other server
- **THEN** the client SHALL ignore the command

### Requirement: Heartbeat and Liveness
The server SHALL send a WebSocket ping every 15 seconds and SHALL close a connection when, at a heartbeat check, no frame of any kind has been received from the client for more than 30 seconds.

#### Scenario: Responsive client
- **WHEN** a client answers pings with pongs
- **THEN** the server SHALL keep the connection open

#### Scenario: Silent client
- **WHEN** a client sends no frame, including pongs, for more than 30 seconds
- **THEN** the server SHALL close the connection at the next heartbeat check

### Requirement: Disconnect Handling
When a connection ends through a close frame, a transport error, or heartbeat timeout, the server SHALL stop delivering events and terminal output to it. A disconnect MUST NOT terminate any session and MUST NOT change workspace state, and other clients SHALL NOT be notified of it.

#### Scenario: Client closes the window
- **WHEN** the only connected client disconnects
- **THEN** all sessions SHALL keep running
- **AND** a client that connects later SHALL see the same screens in `mux.welcome`

### Requirement: Client Reconnect
A client whose established connection drops SHALL reconnect automatically with an increasing delay capped at 10 seconds, obtaining a fresh ticket for every attempt (see `access-control`). A client whose connection fails before it ever opened, or that cannot obtain a ticket, SHALL report an authentication error instead of retrying.

#### Scenario: Server restart
- **WHEN** an open connection drops because the server restarted
- **THEN** the client SHALL fetch a new ticket and reconnect after a delay of at most 10 seconds

#### Scenario: Rejected handshake
- **WHEN** the upgrade is refused before the connection opens
- **THEN** the client SHALL report an authentication error and SHALL NOT retry automatically

### Requirement: Listening Ports
On Linux, while at least one WebSocket client is connected, the server SHALL check every 2 seconds which TCP ports processes of its sessions listen on, where a session's processes are its shell and all their descendants, and the listener's address is loopback (`127.0.0.0/8`, `::1`, or a v4-mapped loopback address) or a wildcard (`0.0.0.0`, `::`). When that list differs from the last one sent, the server SHALL send every client `{"type": "ports.changed", "ports": [{"port", "sessionId", "process"}]}` with the full list, ordered by port, each port once. A client that connects SHALL receive the current list right after its `mux.welcome`. On other systems the server SHALL NOT send the message. A client SHALL ignore server messages whose type it does not know.

#### Scenario: Dev server started in a pane
- **WHEN** a process started in a session's shell begins listening on `127.0.0.1:5173`
- **THEN** within a few seconds every connected client receives `ports.changed` listing port 5173 with that session's id and the process's name

#### Scenario: Server stopped
- **WHEN** that process exits
- **THEN** the clients receive `ports.changed` without port 5173

#### Scenario: Listener on a specific address
- **WHEN** a process of a session listens only on a non-loopback address such as `10.0.0.5:8080`
- **THEN** port 8080 is not listed

### Requirement: Job Snapshots And Background Rendering
Snapshots SHALL identify job panes and their lifecycle, including failed, exited and interrupted jobs. Applying a background job update SHALL preserve each client's selected screen, surviving active tabs and keyboard focus. Clients SHALL display terminal-state job panes without creating a replacement process. New local screens SHALL be discovered from authoritative snapshots, not optimistic duplicate creation.

#### Scenario: User types while a background job appears
- **WHEN** a background job creates a tab, split or local screen while the user types in another pane
- **THEN** the new job appears in its authoritative location and subsequent typing still reaches the original pane

#### Scenario: Failed job after reload
- **WHEN** a client reloads a screen containing a failed or interrupted job
- **THEN** it shows that outcome and sends no request to launch a shell or rerun the job

### Requirement: Job Updates Respect Connection Boundaries
A job update SHALL be resolved only within its sending server's connection. A host update SHALL synchronize already-open host screens but SHALL not automatically open a previously unopened remote screen or a local screen. New remote job screens SHALL be discoverable through the existing screen-list/reopen workflow when the host directory is next refreshed or the connection re-established.

#### Scenario: Job created on a remote host
- **WHEN** a host CLI creates a job in an open remote screen or in a new remote screen
- **THEN** the open screen updates without local focus changes, or the new screen is offered for reopening after the next host-directory refresh/reconnect, and no local pane is changed

### Requirement: Window Layout Changes Reach the Server
The server SHALL announce `layout-arrange-v1` in `mux.welcome.capabilities`. When the user moves a tab or a tab group by drag and drop, or drags a divider, on a screen of a server that announced it, a window SHALL send `arrange_panes` with the arrangement of that screen's grid (its rows, columns and tab groups with their panes, tab order, active tabs and sizes; floating groups are not part of it) once the gesture is done, unless the arrangement equals the last one it received in a snapshot or sent for that screen. It SHALL send none while it applies a snapshot to that screen, while a group of the screen is maximized and while the screen has a group in a popout window, and none to a server that did not announce the capability. A tab selected without a drag SHALL NOT send `arrange_panes`; `select_tab` reports it (see Selected Tabs Reach the Server). On `mux.op_failed` for an `arrange_panes` op, the window SHALL send `mux.resync` for its screen. A window SHALL NOT send `move_pane` for a drag.

#### Scenario: Capability announced
- **WHEN** a client connects
- **THEN** `mux.welcome.capabilities` contains `layout-arrange-v1`

#### Scenario: Tab dragged into another group
- **WHEN** the user drags the tab of pane `p2` into the tab group of `p1` on a screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends `arrange_panes` whose grid holds `p1` and `p2` in one slot
- **AND** after a reconnect the screen shows `p2` in that tab group

#### Scenario: Host without the capability
- **WHEN** the user drags a tab on a screen of a host whose `mux.welcome` has no `layout-arrange-v1`
- **THEN** the window sends neither `arrange_panes` nor `move_pane`

#### Scenario: Rejected arrangement
- **WHEN** the window receives `mux.op_failed` for an `arrange_panes` it sent for screen `s1`
- **THEN** it sends `mux.resync` for `s1` and shows the snapshot's layout

### Requirement: Selected Tabs Reach the Server
The server SHALL announce `tab-select-v1` in `mux.welcome.capabilities`. When the user selects a tab on a screen of a server that announced it, a window SHALL send, once the selection is done, `select_tab` for every tab group of that screen that holds two or more tabs, is in the grid or floating, and shows a tab other than the one the server has as its slot's active pane, as far as the window knows from the last snapshot it applied and the ops it sent since. It SHALL send none while it applies a snapshot to that screen, none for a group in a popout window, and none to a server that did not announce the capability. The server SHALL send no `mux.nudge` and no `mux.op_failed` for `select_tab`. When a window builds a screen's layout from a snapshot without a layout it showed before, each tab group SHALL show its slot's active pane, or the slot's first pane when the slot names none.

#### Scenario: Capability announced
- **WHEN** a client connects
- **THEN** `mux.welcome.capabilities` contains `tab-select-v1`

#### Scenario: Tab selected and the app restarted
- **WHEN** a tab group shows `p3` of `[p1, p2, p3]`, the user selects `p1` on a screen of a server that announced `tab-select-v1`, and the window is then built anew
- **THEN** the window sent `select_tab` for `p1`
- **AND** the rebuilt window shows `p1` in that tab group

#### Scenario: The tab the server already has
- **WHEN** the user selects a group's tab that the server has as that slot's active pane, and every other group with two or more tabs shows the server's active pane
- **THEN** the window sends nothing

#### Scenario: Host without the capability
- **WHEN** the user selects a tab on a screen of a host whose `mux.welcome` has no `tab-select-v1`
- **THEN** the window sends neither `select_tab` nor `arrange_panes`

#### Scenario: A slot without an active pane
- **WHEN** a window builds a screen anew from a snapshot whose slot holds `[p1, p2]` and names no active pane
- **THEN** the tab group shows `p1`
