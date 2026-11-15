## ADDED Requirements

### Requirement: Connection Liveness
Liveness SHALL belong to the carrier of a connection; the session messages SHALL carry no heartbeat of their own.

A native connection SHALL send a transport ping every 15 seconds while none of its pings is unanswered, SHALL answer every ping of its peer with a pong carrying the same nonce, and SHALL end the connection when, at one of its 15-second checks, no pong matching its outstanding ping has arrived for 30 seconds or more since the last matching pong or, before the first, since the connection was accepted. Other frames SHALL NOT count as an answer. A peer that falls silent is therefore ended within 45 seconds.

When a connection ends by liveness, the server SHALL release it as on any other disconnect (see Disconnect Handling): its client registration, its editor and file registrations and the streams it attached.

#### Scenario: Responsive client
- **WHEN** a native client sends no application message for several minutes but answers transport pings
- **THEN** the server SHALL keep the connection and its registration

#### Scenario: Silent client
- **WHEN** a native client stops answering transport pings
- **THEN** the server SHALL end the connection within 45 seconds and release its registration

#### Scenario: Peer of the previous release
- **WHEN** a native client or host of the previous release connects
- **THEN** transport pings decide its liveness as before; no capability is involved, since session messages never carried a ping on a native connection

## MODIFIED Requirements

### Requirement: Shared Connection for Control and Terminal I/O
One authorized native application connection MUST carry both workspace control messages and terminal session messages. The server SHALL route every application text message whose `type` starts with `mux.` to workspace control, and every other application text message to the session protocol defined in `terminal-sessions`.

#### Scenario: Mux message routing
- **WHEN** a client sends a text frame with `type` `mux.resync`
- **THEN** the server SHALL handle it as a workspace control message

#### Scenario: Session message routing
- **WHEN** a client sends a text frame with `type` `input` on the same connection
- **THEN** the server SHALL handle it as a session message as defined in `terminal-sessions`

### Requirement: Message Encoding
Client-to-server messages SHALL be JSON objects sent as application text messages, discriminated by a string `type` field. Server-to-client events other than terminal output SHALL be JSON objects sent as application text messages, discriminated by `type`; every `mux.*` event and every `command` event MUST carry a unique event `id` and a `timestamp` in Unix milliseconds.

#### Scenario: Server event envelope
- **WHEN** the server sends a `mux.nudge` event
- **THEN** the event SHALL be a JSON text message with `type` `mux.nudge`, an `id`, and a `timestamp`

### Requirement: Binary Terminal Output Frames
The server SHALL deliver terminal output as binary application messages with the layout `[1 byte: session ID length N][N bytes: session ID][payload bytes]`. The server SHALL ignore binary messages sent by a client.

#### Scenario: Output frame layout
- **WHEN** the server delivers output bytes for session `sess-abc`
- **THEN** it SHALL send one binary message whose first byte is 8, followed by the bytes of `sess-abc`, followed by the output bytes

#### Scenario: Client sends a binary frame
- **WHEN** a client sends a binary message
- **THEN** the server SHALL discard it without a response and keep the connection open

### Requirement: Welcome on Connect
Immediately after a completed native service negotiation, without waiting for any client message, the server SHALL send `mux.welcome` containing the connection's `clientId`, the list of screens in workspace order (each with `id` and optional `name`), and `serverMode` set to `embedded` or `daemon` (see `server-modes`).

#### Scenario: Welcome lists existing screens
- **WHEN** a client connects to a server whose workspace holds screens `C`, `A`, `B` in that order
- **THEN** `mux.welcome.screens` SHALL list `C`, `A`, `B` in that order

#### Scenario: Empty workspace
- **WHEN** a client connects to a server with no screens
- **THEN** `mux.welcome.screens` SHALL be an empty list

### Requirement: Pane Spawn Notification
When an op submitted over a client connection successfully starts a session for a pane, the server SHALL send `mux.pane_spawned` with that `paneId` and the new `sessionId` to the originating client, and SHALL enqueue it before processing that client's next message. A failed spawn SHALL send `mux.op_failed` with the pane and actual screen identifiers instead, and SHALL never advertise the absent session as spawned. A client receiving `mux.pane_spawned` SHALL connect the pane to the given session through the session protocol (see `terminal-sessions`) instead of creating a session itself.

#### Scenario: Split creates a session
- **WHEN** a client sends a `mux.op` that creates pane `p2` with a new session
- **THEN** the client SHALL receive `mux.pane_spawned` with `paneId` `p2` and the server-assigned `sessionId`

#### Scenario: Job program cannot start
- **WHEN** a job op is accepted but execution fails
- **THEN** the originating client receives failure rather than a successful spawn event, and synchronized job state reports `failed`

### Requirement: Change Propagation to Other Clients
For an accepted layout op on a client connection, the server SHALL notify every other connected client of the screens the op changed, added or removed using `mux.nudge` with reason `mux_op` and their actual screen IDs. All successful `mux.pane_spawned` events SHALL precede the op's nudges; each affected screen SHALL be nudged at most once per op. The originator SHALL receive no success nudge. The documented `select_tab` exception remains silent. A workspace order or active-screen change with no changed screen SHALL produce one nudge without a `screenId`. When the server adds an agent's reply, or starts or ends a wait for a review (see `viewer-annotations`), it SHALL send every connected client `mux.nudge` with reason `mux_op` for the viewer's screen.

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

#### Scenario: Viewer opened and closed by a client
- **WHEN** client A opens a viewer pane on screen `s1` with `open_viewer` and later closes it with `close_pane` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1` after each of the two ops
- **AND** client A SHALL receive neither

#### Scenario: Annotation added by a client
- **WHEN** client A sends `put_annotations` on screen `s1` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive it

#### Scenario: Reply of an agent
- **WHEN** a pane process adds a reply to an annotation of a viewer pane on screen `s1` through the control API while clients A and B are connected
- **THEN** A and B SHALL each receive `mux.nudge` with reason `mux_op` and `screenId` `s1`

#### Scenario: Layout edit without process effects
- **WHEN** client A changes split sizes, moves, floats or docks a pane, moves floating bounds, or renames a screen without starting or stopping a session
- **THEN** client B receives one nudge for the affected screen and client A receives none

#### Scenario: Workspace-only mutation
- **WHEN** client A changes screen order or active screen without changing a screen's state
- **THEN** client B receives one `mux_op` nudge without `screenId` and client A receives none

#### Scenario: Closing a screen without sessions
- **WHEN** client A closes an empty or viewer-only screen
- **THEN** client B receives a nudge naming the removed screen even though the op has no process effects

#### Scenario: Generated and affected screen identifiers
- **WHEN** an op creates a server-generated screen or changes both source and target screens
- **THEN** its nudges identify the actual affected screens rather than an empty or unrelated request envelope ID, without duplicates

### Requirement: Client Reconnect
A client whose established connection drops SHALL reconnect with an increasing delay capped at 10 seconds. The client SHALL reverify its assigned endpoint/instance and negotiate a fresh connection without a credential. Authorization or compatibility rejection SHALL stop automatic retry with a specific error; native transport unavailability SHALL not be reported as a login error. Reconnect SHALL discard old input and mutations and resync as defined in client-connections.

#### Scenario: Server restart
- **WHEN** a connection drops because the server restarted
- **THEN** the client reconnects to the verified new instance with a fresh native negotiation and resynchronizes

#### Scenario: Rejected handshake
- **WHEN** authorization or protocol compatibility is rejected
- **THEN** the client reports that cause without automatic retry or a fallback listener

#### Scenario: Native endpoint temporarily absent
- **WHEN** an established native daemon connection drops while the daemon restarts
- **THEN** the client reports reconnection and retries discovery without asking for a token or password

### Requirement: Listening Ports
On Linux, while at least one client is connected, the server SHALL check every 2 seconds which TCP ports processes of its sessions listen on, where a session's processes are its shell and all their descendants, and the listener's address is loopback (`127.0.0.0/8`, `::1`, or a v4-mapped loopback address) or a wildcard (`0.0.0.0`, `::`). When that list differs from the last one sent, the server SHALL send every client `{"type": "ports.changed", "ports": [{"port", "sessionId", "process"}]}` with the full list, ordered by port, each port once. A client that connects SHALL receive the current list right after its `mux.welcome`. On other systems the server SHALL NOT send the message. A client SHALL ignore server messages whose type it does not know.

#### Scenario: Dev server started in a pane
- **WHEN** a process started in a session's shell begins listening on `127.0.0.1:5173`
- **THEN** within a few seconds every connected client receives `ports.changed` listing port 5173 with that session's id and the process's name

#### Scenario: Server stopped
- **WHEN** that process exits
- **THEN** the clients receive `ports.changed` without port 5173

#### Scenario: Listener on a specific address
- **WHEN** a process of a session listens only on a non-loopback address such as `10.0.0.5:8080`
- **THEN** port 8080 is not listed

### Requirement: Window Layout Changes Reach the Server
The server SHALL announce `layout-arrange-v1` in `mux.welcome.capabilities`. `arrange_panes` carries the arrangement of a screen's grid (its rows, columns and tab groups with their panes, tab order, active tabs and sizes; floating groups are not part of it). Workspace windows use precise pane-move and split-resize operations for their drags (see `workspace-model`), except for the move of a whole tab group, which they send as one `arrange_panes` (see `workspace-model`, Native Tab Group Move), and only to a server that announced the capability. A tab selected without a drag SHALL NOT send `arrange_panes`; `select_tab` reports it (see Selected Tabs Reach the Server). On `mux.op_failed` for an `arrange_panes` op, the window SHALL send `mux.resync` for its screen.

#### Scenario: Capability announced
- **WHEN** a client connects
- **THEN** `mux.welcome.capabilities` contains `layout-arrange-v1`

#### Scenario: Tab dragged into another group
- **WHEN** the user drags, in a native workspace window, the tab of pane `p2` into the tab group of `p1` on a screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends a precise pane move (see `workspace-model`) and no `arrange_panes`
- **AND** after a reconnect the screen shows `p2` in that tab group

#### Scenario: Host without the capability
- **WHEN** the user drops a whole tab group in a native workspace window on a screen of a host whose `mux.welcome` has no `layout-arrange-v1`
- **THEN** the window sends no `arrange_panes`

#### Scenario: Rejected arrangement
- **WHEN** a workspace window receives `mux.op_failed` for an `arrange_panes` it sent for screen `s1`
- **THEN** it sends `mux.resync` for `s1` and shows the snapshot's layout

#### Scenario: Native group move
- **WHEN** the user drops a whole tab group in a native workspace window on a screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends one `arrange_panes` for that screen and no `move_pane` or `move_tab`

## REMOVED Requirements

### Requirement: WebSocket Endpoint
**Reason**: The `/ws` endpoint was the browser gateway's way to carry the application protocol, and the gateway is removed. Every client now reaches a service over a native connection (see `client-connections`), which carries the same application messages without a WebSocket upgrade.

**Migration**: Clients of `/ws` use a native connection: the desktop app, `splitty attach`, or `desktop-backend --transport native-v1` for a launcher.

### Requirement: Heartbeat and Liveness
**Reason**: Restated as Connection Liveness without the browser gateway's WebSocket pings, which go with the gateway; the native transport rules are unchanged.
