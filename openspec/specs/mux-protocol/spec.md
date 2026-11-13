# mux-protocol Specification

## Purpose
The real-time WebSocket protocol between GUI clients and the server: how a connection is established and kept alive, how messages are framed, how clients learn and resynchronize workspace state, how layout ops are submitted and their outcomes reported, and how changes and server commands reach connected clients. Payload shapes are defined in `docs/protocol.ts`; layout op semantics live in `workspace-model`, session message semantics in `terminal-sessions`.

## Requirements

### Requirement: WebSocket Endpoint
An explicit browser gateway SHALL expose the real-time protocol at `/ws`; native connections SHALL carry the same application protocol without a WebSocket upgrade. The gateway SHALL serve its WebSocket endpoint and SHALL only complete the upgrade for requests that are authorized as defined in `access-control` (allowed origin and a single-use `ticket` query parameter).

#### Scenario: Authorized upgrade
- **WHEN** a client opens `/ws?ticket=<ticket>` from an allowed origin with a valid, unused ticket
- **THEN** the server SHALL complete the WebSocket upgrade
- **AND** the server SHALL send `mux.welcome` on the new connection after opening its authorized service stream

#### Scenario: Unauthorized upgrade
- **WHEN** a client opens `/ws` without a valid ticket or from a disallowed origin
- **THEN** the server SHALL reject the request with an HTTP error before upgrading
- **AND** no protocol message SHALL be sent

### Requirement: Shared Connection for Control and Terminal I/O
One authorized application connection, native or WebSocket, MUST carry both workspace control messages and terminal session messages. The server SHALL route every application text message whose `type` starts with `mux.` to workspace control, and every other application text message to the session protocol defined in `terminal-sessions`.

#### Scenario: Mux message routing
- **WHEN** a client sends a text frame with `type` `mux.resync`
- **THEN** the server SHALL handle it as a workspace control message

#### Scenario: Session message routing
- **WHEN** a client sends a text frame with `type` `input` on the same connection
- **THEN** the server SHALL handle it as a session message as defined in `terminal-sessions`

### Requirement: Message Encoding
Client-to-server messages SHALL be JSON objects sent as text messages, mapped to WebSocket text frames for browser clients, discriminated by a string `type` field. Server-to-client events other than terminal output SHALL be JSON objects sent as text frames, discriminated by `type`; every `mux.*` event and every `command` event MUST carry a unique event `id` and a `timestamp` in Unix milliseconds.

#### Scenario: Server event envelope
- **WHEN** the server sends a `mux.nudge` event
- **THEN** the event SHALL be a JSON text frame with `type` `mux.nudge`, an `id`, and a `timestamp`

### Requirement: Binary Terminal Output Frames
The server SHALL deliver terminal output as binary application messages, mapped to binary WebSocket frames for browser clients with the layout `[1 byte: session ID length N][N bytes: session ID][payload bytes]`. The server SHALL ignore binary frames sent by a client.

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
Immediately after an authorized WebSocket upgrade or completed native service negotiation, without waiting for any client message, the server SHALL send `mux.welcome` containing the connection's `clientId`, the list of screens in workspace order (each with `id` and optional `name`), and `serverMode` set to `embedded` or `daemon` (see `server-modes`).

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
A client MUST treat the server's workspace state as authoritative after connecting: on `mux.welcome` it SHALL create any listed screen it does not have locally and SHALL send `mux.resync` for every listed screen. A client SHALL build every screen listed in `mux.welcome` from its `mux.screen_snapshot`, also when it has prepared the screen locally before the snapshot arrives, and MUST NOT send `create_screen` for such a screen in its place. (Re-creating a screen that later ran empty stays allowed, see `workspace-model`.) Before the first `mux.welcome` of a connection a client MUST NOT send `create_screen`. It SHALL create an initial screen of its own only when the welcome lists no screens, with `onlyIfEmpty: true` (see `workspace-model`, First Screen of an Empty Workspace), and SHALL show only the listed screens when the welcome lists any. When the server refuses that initial screen, the client SHALL drop it and show the screens the server has instead. On a `mux.nudge` that carries a `screenId`, a client SHALL send `mux.resync` for that screen.

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
- **THEN** the client SHALL create one screen with `create_screen` and `onlyIfEmpty: true`

#### Scenario: Two windows start against an empty workspace
- **WHEN** two windows start together, both welcomes list no screens and both send their initial `create_screen` with `onlyIfEmpty: true`
- **THEN** the server holds exactly one screen
- **AND** the window whose create was refused drops its own screen and shows the one the server holds

#### Scenario: Older server
- **WHEN** a server without `screen-names-v1` ignores `onlyIfEmpty` and two windows start together against its empty workspace
- **THEN** each window's initial screen is created, as before

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
When the server rejects a parsed layout op during validation or application, it SHALL send `mux.op_failed` to the originating client only, carrying the op `type`, the `screenId` and a non-empty error. The event SHALL retain `newPaneId` as `paneId` for pane-creating ops, and the viewer's `paneId` for annotation ops. The window SHALL request `mux.resync` when that failure names a screen, including non-pane-creating layout ops. The silent `select_tab` exception in Selected Tabs Reach the Server and the malformed/unknown-message policy remain unchanged.

#### Scenario: Rejected pane-creating op
- **WHEN** a client sends a `mux.op` with `newPaneId` `p9` that the server rejects
- **THEN** the client SHALL receive `mux.op_failed` with `paneId` `p9` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected arrangement
- **WHEN** a client sends `arrange_panes` on screen `s1` that misses one of its panes
- **THEN** the client SHALL receive `mux.op_failed` with `op` `arrange_panes`, `screenId` `s1` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected annotation
- **WHEN** a client sends `put_annotations` for viewer pane `v1` on screen `s1` that would give the pane a 101st annotation
- **THEN** the client SHALL receive `mux.op_failed` with `op` `put_annotations`, `screenId` `s1`, `paneId` `v1` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected reply
- **WHEN** a client sends `put_replies` for viewer pane `v1` on screen `s1` with the ID of an agent's reply
- **THEN** the client SHALL receive `mux.op_failed` with `op` `put_replies`, `screenId` `s1`, `paneId` `v1` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected layout edit without a new pane
- **WHEN** a client sends a parsed `resize_split`, `move_pane`, `float_pane`, `move_float`, `dock_pane` or workspace op that is rejected without naming `newPaneId`
- **THEN** it receives `mux.op_failed` with the op and screen identifiers and requests that screen's snapshot
- **AND** other clients receive neither that failure nor an op-change nudge

#### Scenario: Failure without a screen identifier
- **WHEN** a window receives a failure without a usable `screenId`
- **THEN** it reports the failure to its consumers without sending an invalid screen resync

### Requirement: Change Propagation to Other Clients
For an accepted layout op on a native or browser connection, the server SHALL notify every other connected client of the screens the op changed, added or removed using `mux.nudge` with reason `mux_op` and their actual screen IDs. All successful `mux.pane_spawned` events SHALL precede the op's nudges; each affected screen SHALL be nudged at most once per op. The originator SHALL receive no success nudge. The documented `select_tab` exception remains silent. A workspace order or active-screen change with no changed screen SHALL produce one nudge without a `screenId`. When the server adds an agent's reply, or starts or ends a wait for a review (see `viewer-annotations`), it SHALL send every connected client `mux.nudge` with reason `mux_op` for the viewer's screen.

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
Liveness SHALL belong to the carrier of a connection; the session messages SHALL carry no heartbeat of their own.

A native connection SHALL send a transport ping every 15 seconds while none of its pings is unanswered, SHALL answer every ping of its peer with a pong carrying the same nonce, and SHALL end the connection when, at one of its 15-second checks, no pong matching its outstanding ping has arrived for 30 seconds or more since the last matching pong or, before the first, since the connection was accepted. Other frames SHALL NOT count as an answer. A peer that falls silent is therefore ended within 45 seconds.

The browser gateway SHALL send a WebSocket ping to the browser every 15 seconds while none is unanswered and SHALL close the browser connection 30 seconds after the last matching pong or, before the first, after the upgrade. Closing it SHALL end the gateway's native connection to the service for that browser.

When a connection ends by liveness, the server SHALL release it as on any other disconnect (see Disconnect Handling): its client registration, its editor and file registrations and the streams it attached.

#### Scenario: Responsive client
- **WHEN** a native client sends no application message for several minutes but answers transport pings
- **THEN** the server SHALL keep the connection and its registration

#### Scenario: Silent client
- **WHEN** a native client stops answering transport pings
- **THEN** the server SHALL end the connection within 45 seconds and release its registration

#### Scenario: Silent browser
- **WHEN** a browser stops answering the gateway's pings while the gateway's native connection to the service stays healthy
- **THEN** the gateway SHALL close the browser connection 30 seconds after the last matching pong, and the service SHALL release that browser's session

#### Scenario: Peer of the previous release
- **WHEN** a native client or host of the previous release connects
- **THEN** transport pings decide its liveness as before; no capability is involved, since session messages never carried a ping on a native connection

### Requirement: Disconnect Handling
When a connection ends through a close frame, a transport error, or heartbeat timeout, the server SHALL stop delivering events and terminal output to it. A disconnect MUST NOT terminate any session and MUST NOT change workspace state, and other clients SHALL NOT be notified of it.

#### Scenario: Client closes the window
- **WHEN** the only connected client disconnects
- **THEN** all sessions SHALL keep running
- **AND** a client that connects later SHALL see the same screens in `mux.welcome`

### Requirement: Client Reconnect
A client whose established connection drops SHALL reconnect with an increasing delay capped at 10 seconds. A browser client SHALL obtain a fresh ticket on each attempt; a native client SHALL reverify its assigned endpoint/instance and negotiate a fresh connection without browser credentials. Authorization or compatibility rejection SHALL stop automatic retry with a specific error; native transport unavailability SHALL not be mislabeled a browser login error. Reconnect SHALL discard old input and mutations and resync as defined in client-connections.

#### Scenario: Server restart
- **WHEN** a connection drops because the server restarted
- **THEN** the client reconnects to the verified new instance with fresh browser authorization or native negotiation and resynchronizes

#### Scenario: Rejected handshake
- **WHEN** authorization or protocol compatibility is rejected
- **THEN** the client reports that cause without automatic retry or a fallback listener

#### Scenario: Native endpoint temporarily absent
- **WHEN** an established native daemon connection drops while the daemon restarts
- **THEN** the client reports reconnection and retries discovery without displaying a browser-token prompt

### Requirement: Listening Ports
On Linux, while at least one native or WebSocket client is connected, the server SHALL check every 2 seconds which TCP ports processes of its sessions listen on, where a session's processes are its shell and all their descendants, and the listener's address is loopback (`127.0.0.0/8`, `::1`, or a v4-mapped loopback address) or a wildcard (`0.0.0.0`, `::`). When that list differs from the last one sent, the server SHALL send every client `{"type": "ports.changed", "ports": [{"port", "sessionId", "process"}]}` with the full list, ordered by port, each port once. A client that connects SHALL receive the current list right after its `mux.welcome`. On other systems the server SHALL NOT send the message. A client SHALL ignore server messages whose type it does not know.

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
The server SHALL announce `layout-arrange-v1` in `mux.welcome.capabilities`. When the user moves a tab or a tab group by drag and drop, or drags a divider, on a screen of a server that announced it, a browser workspace window SHALL send `arrange_panes` with the arrangement of that screen's grid (its rows, columns and tab groups with their panes, tab order, active tabs and sizes; floating groups are not part of it) once the gesture is done, unless the arrangement equals the last one it received in a snapshot or sent for that screen. It SHALL send none while it applies a snapshot to that screen, while a group of the screen is maximized and while the screen has a group in a popout window, and none to a server that did not announce the capability. A tab selected without a drag SHALL NOT send `arrange_panes`; `select_tab` reports it (see Selected Tabs Reach the Server). On `mux.op_failed` for an `arrange_panes` op, the window SHALL send `mux.resync` for its screen. A browser workspace window SHALL NOT send `move_pane` for a drag. Native workspace windows use precise pane-move and split-resize operations instead of reporting complete arrangements (see `workspace-model`).

#### Scenario: Capability announced
- **WHEN** a client connects
- **THEN** `mux.welcome.capabilities` contains `layout-arrange-v1`

#### Scenario: Tab dragged into another group
- **WHEN** the user drags, in a browser workspace window, the tab of pane `p2` into the tab group of `p1` on a screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends `arrange_panes` whose grid holds `p1` and `p2` in one slot
- **AND** after a reconnect the screen shows `p2` in that tab group

#### Scenario: Host without the capability
- **WHEN** the user drags a tab in a browser workspace window on a screen of a host whose `mux.welcome` has no `layout-arrange-v1`
- **THEN** the window sends neither `arrange_panes` nor `move_pane`

#### Scenario: Rejected arrangement
- **WHEN** the browser workspace window receives `mux.op_failed` for an `arrange_panes` it sent for screen `s1`
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

### Requirement: Review Answers
A client SHALL answer a wait for a review (see `viewer-annotations`, Waiting For A Review) with the message `viewer.review_answer`, carrying the `screenId`, the viewer pane's `paneId`, the wait's `waitId`, the `outcome`, `approved` or `changes_requested`, and the `review`, 1 byte to 1 MiB of UTF-8, which is optional with `approved` and required with `changes_requested`. The server SHALL settle the wait with the first answer whose `waitId` names it, end the wait, and answer every `viewer.review_answer` to its sender only with `viewer.review_answered`, carrying the `paneId`, the `waitId` and `accepted`, true only for the answer that settled the wait. An answer for a wait that has ended, for another pane, or that breaks these rules SHALL change nothing and be answered with `accepted` false.

#### Scenario: The first answer
- **WHEN** client A sends `viewer.review_answer` with outcome `changes_requested` for the wait of viewer pane `v1`, and then client B sends one with outcome `approved` for the same wait
- **THEN** A receives `viewer.review_answered` with `accepted` true, B receives it with `accepted` false, and the waiting process receives A's review

#### Scenario: An approval with notes
- **WHEN** a client answers the wait of `v1` with outcome `approved` and a review
- **THEN** the client receives `accepted` true, and the waiting process receives the approval with the review

#### Scenario: A request for changes without a review
- **WHEN** a client answers a wait with outcome `changes_requested` and no `review`
- **THEN** the client receives `accepted` false, and the wait goes on

#### Scenario: A stale answer
- **WHEN** a client answers a wait whose process has already stopped waiting
- **THEN** the client receives `viewer.review_answered` with `accepted` false, and nothing changes

### Requirement: Template Operation Capability
`mux.welcome` SHALL advertise `server-templates-v1` when template application and extraction are supported. A window SHALL disable these actions without the capability and offer a hub-authorized update or manual update guidance for an older host, without direct session spawning as a fallback.

#### Scenario: Older host
- **WHEN** a host welcome lacks `server-templates-v1`
- **THEN** template application and extraction are unavailable and a hub-authorized update or manual update guidance is offered

### Requirement: Template Operation Results
`apply_template` SHALL use the payload in docs/protocol.ts on every session transport. Each successful launch SHALL produce `mux.pane_spawned` before other windows are nudged. Rejection SHALL produce `mux.op_failed` for the screen, and an individual launch failure SHALL also name its pane. The requesting window SHALL resynchronize after failure.

#### Scenario: Result ordering and rejection
- **WHEN** a template op succeeds or fails
- **THEN** successful spawn results precede the screen nudge and rejection triggers resynchronization without a client-created session

### Requirement: Template Extraction Over Session Connections
A window SHALL request extraction from its screen’s own connection using `mux.extract_template` and receive `mux.template_extracted` with its request ID and either a template or an error. This SHALL use the same semantics as authenticated HTTP/native extraction and SHALL never fall back to a different server.

#### Scenario: Extract a remote screen
- **WHEN** a window extracts a screen on a host connection
- **THEN** the request and response stay on that connection and a disconnect fails the pending extraction

### Requirement: Server Derived Pane State
The server SHALL advertise serverDerivedPaneState. Session snapshots SHALL carry displayTitle and busy, and ordinary terminal pane titles SHALL match their session display title. Metadata SHALL refresh without attached clients. Clients SHALL use these values and leave automatic exit closure to the server; older servers retain legacy client behavior.

#### Scenario: Reconnect to an unattended pane
- **WHEN** a process changes while no window shows its pane and a client later resynchronizes
- **THEN** the snapshot gives its current displayTitle and busy state and the pane title matches

#### Scenario: Older host
- **WHEN** a host does not advertise serverDerivedPaneState
- **THEN** the client retains its legacy title, launch and close-on-exit handling

### Requirement: Ordered Workspace Catalog
A service announcing `workspace-catalog-v1` SHALL answer `mux.workspace_resync` on the existing session stream with `mux.workspace_snapshot`. Its ordered `screens` and nullable `activeScreenId` hint SHALL describe one coherent catalog, without pane layouts. Duplicate IDs SHALL appear once at their first position; an active ID absent from the catalog SHALL be returned as null. The request SHALL be processed after preceding operations on that connection. Payloads are defined in `docs/protocol.ts`.

#### Scenario: Catalog after lifecycle operations
- **WHEN** a client sends create, rename or close operations followed by `mux.workspace_resync`
- **THEN** the response reflects those completed operations, including the removal of the last screen
- **AND** the same stream continues delivering terminal output in order

#### Scenario: Coherent order and active screen
- **WHEN** a workspace catalog is requested during concurrent workspace edits
- **THEN** the returned screen names, order and active-screen identifier belong to one coherent state
- **AND** duplicate order entries appear once and an active identifier absent from the returned catalog becomes null

#### Scenario: Additive capability
- **WHEN** the welcome does not announce `workspace-catalog-v1`
- **THEN** a client SHALL NOT send `mux.workspace_resync` or infer support from successful transport negotiation
- **AND** existing per-screen resync behavior remains unchanged

### Requirement: Exact Tab Placement Capability
A server supporting `move_tab` SHALL announce `tab-placement-v1` in `mux.welcome.capabilities`. A client SHALL NOT send exact tab placement without that capability. Existing `move_pane` semantics SHALL remain unchanged. Accepted placement SHALL use ordinary layout-change propagation and rejected placement SHALL use `mux.op_failed`, followed by client resynchronization.

#### Scenario: Supported exact placement
- **WHEN** a client connects to a supporting service and places a tab
- **THEN** the welcome announces `tab-placement-v1` and the operation is processed in stream order with ordinary layout propagation

#### Scenario: Older service
- **WHEN** the welcome lacks `tab-placement-v1`
- **THEN** exact placement is unavailable and the client sends neither `move_tab` nor a lossy fallback operation for that intent

#### Scenario: Rejected exact placement
- **WHEN** a parsed `move_tab` names an invalid source, destination or anchor
- **THEN** only its sender receives `mux.op_failed` naming `move_tab` and its Screen, and the client requests authoritative state
- **AND** other clients receive no change nudge

#### Scenario: Legacy move remains compatible
- **WHEN** an existing client sends `move_pane` with `targetSlotId`
- **THEN** the moved pane is appended and activated, including a same-slot move

### Requirement: Program Status Capability
A server that reads program status (see `program-status`) SHALL list `program-status-v1` in `mux.welcome.capabilities`. A client SHALL show program status only for a connection whose `mux.welcome` lists it, and SHALL otherwise behave as before.

#### Scenario: Current server
- **WHEN** a client connects to a server of this version
- **THEN** `mux.welcome.capabilities` lists `program-status-v1`

#### Scenario: Older server
- **WHEN** a client connects to a server whose `mux.welcome` does not list `program-status-v1`
- **THEN** the client shows no program status for that connection and works as before

### Requirement: Program Status Events
The server SHALL send every connection `{"type": "program_status.changed", "sessionId", "paneId"?, "screenId"?, "status", "notify"?}`, where `status` is the session's root record or `null` and the pane and screen are the session's, if it has one (shapes in `docs/protocol.ts`): right after `mux.welcome` for each session that holds a root record, and when a root record changes, goes, or its pane moves to another screen. Per session, events SHALL be at least 100 ms apart and carry the latest record. A client SHALL ignore server messages whose type it does not know.

#### Scenario: Status on connect
- **WHEN** a session holds a `blocked` root record and a client connects
- **THEN** right after `mux.welcome` the client receives `program_status.changed` for that session with the state `blocked`, the session's `paneId` and `screenId`, and no `notify`

#### Scenario: Every connection receives the change
- **WHEN** two clients are connected, only one of them streams a session, and the session's root record changes
- **THEN** both clients receive `program_status.changed` for that session

#### Scenario: Paced changes
- **WHEN** a program writes 100 reports within one second
- **THEN** a client receives no two events for that session less than 100 ms apart, and the last carries the state of the last report

#### Scenario: Record cleared
- **WHEN** a session's program clears its root record
- **THEN** every client receives `program_status.changed` for that session with `status` `null`

#### Scenario: Session removed
- **WHEN** a session that holds a `done` root record is closed
- **THEN** every client receives `program_status.changed` for that session with `status` `null`

#### Scenario: Child records only
- **WHEN** a session holds no root record and its program reports a child record
- **THEN** no `program_status.changed` is sent for that session

#### Scenario: Pane moved to another screen
- **WHEN** a pane whose session holds a root record moves to another screen
- **THEN** every client receives `program_status.changed` for that session with the new `screenId` and no `notify`

### Requirement: Close Assessment
A server SHALL list `close-assessment-v1` in `mux.welcome.capabilities` and answer `{"type": "mux.assess_close", "requestId", "scope"}` on the same connection with `{"type": "mux.close_assessed", "requestId", "assessment"}`, or with `error` for a malformed scope (shapes in `docs/protocol.ts`). The `scope` names the panes of one screen, whole screens, or the whole workspace; a screen or pane the server does not have adds nothing. The assessment SHALL list each pane, and each session of a named screen without a pane, that closing would make lose something, with its screen, pane and session IDs, its display title, and what it loses: a terminal's program that runs (`running`) or is still starting (`starting`), a scratchpad that holds text (`scratchpad`), and the number of annotations a viewer pane holds (`annotations`). Managed job panes SHALL NOT count as running. It SHALL carry the totals of these and `confirm`, true exactly when any entry loses something. The answer SHALL reflect the server's state after the preceding messages of that connection and SHALL change nothing. A window SHALL decide whether to ask before a pane, tab, screen or window close and before a template replaces a screen from this answer only, and SHALL send the request on the connection of the screen it closes, so a host's screen is assessed by its host through the hub's relay.

#### Scenario: A busy pane and an idle pane
- **WHEN** a window assesses the panes `p1` and `p2` of screen `s1`, where `p1` runs `vim` and `p2` is an idle shell
- **THEN** the answer lists `p1` with `running` true and not `p2`, and `confirm` is true

#### Scenario: A pane that is still starting
- **WHEN** a window assesses a pane whose session has not started yet
- **THEN** the pane is listed with `starting` true and `confirm` is true

#### Scenario: Nothing to lose
- **WHEN** every pane in the scope is an idle shell without scratchpad text and no viewer holds annotations
- **THEN** the answer lists nothing and `confirm` is false, and the window closes without asking

#### Scenario: The whole workspace
- **WHEN** a window of an embedded server assesses the workspace before it quits
- **THEN** the answer covers the panes and sessions of every screen, shown or not

#### Scenario: Older server
- **WHEN** a window closes a pane or screen of a connection whose `mux.welcome` does not list `close-assessment-v1`, or the assessment fails or does not come
- **THEN** the window asks a generic confirmation before closing, also for an idle pane, and offers no assessment-based text

### Requirement: Notification Notices Capability
A server that classifies its notification events SHALL list `notification-notices-v1` in `mux.welcome.capabilities`. For a connection without it, a window SHALL add for each `process.finished`, `process.idle`, `session.exited` and `notification` event an `info` notification titled `Activity in <process>`, or `Session activity` when the event names no process, while `notifications.enabled` is on, and SHALL derive nothing else from the event; a `program_status.changed` event without `notice` SHALL add no notification.

#### Scenario: Current server
- **WHEN** a client connects to a server of this version
- **THEN** `mux.welcome.capabilities` lists `notification-notices-v1`

#### Scenario: Older server
- **WHEN** a window is connected to a host whose `mux.welcome` does not list `notification-notices-v1` and the host sends `process.finished` for `cargo`, which the user is not watching
- **THEN** the window adds an `info` notification titled `Activity in cargo`, prefixed with the host's name

### Requirement: Classified Notification Events
The server SHALL attach `notice` to every `process.finished`, `process.idle` and `session.exited` event and to every `program_status.changed` event marked `notify`, and to no other `program_status.changed` (shapes in `docs/protocol.ts`). A notice SHALL carry `kind`, `severity` (`info`, `success`, `warning` or `error`), a plain-text `title`, `setting` when a `notifications.*` switch besides `enabled` governs it, and `suppressed` when no window should notify:

- `process.finished`: kind `process.finished`, `info`, `<process> finished`, setting `processFinished`.
- `process.idle`: kind `process.idle`, `info`, `<process> is waiting` or `Command finished`; suppressed `program-status` while the session holds a root program status record.
- `session.exited`: with exit code 0 kind `session.exited`, `info`, `<process> exited`, setting `processExited`; otherwise kind `session.crashed`, `error`, `<process> failed`, setting `processCrashed`, where `<process>` is `session` when none is known; suppressed `user-initiated` when `userInitiated` is true.
- `program_status.changed`: kind `program-status`, `warning` for `blocked`, `success` for `done`, `error` for `error`, setting `programStatus`; the title names the screen (its name, else `Unnamed Screen`, never a label by its place) followed by the state in words and the record's `msg`, else its `title`.

A session SHALL exit user-initiated when the server closed it while its program was still running: on a connection's `close` or when its pane or screen was closed.

#### Scenario: Process finished
- **WHEN** a streamed session's foreground program `cargo` ends and the shell is in the foreground again
- **THEN** every connection receives `process.finished` with a notice of kind `process.finished`, severity `info`, title `cargo finished` and setting `processFinished`

#### Scenario: Session failed
- **WHEN** a streamed session's program exits with code 1 by itself
- **THEN** its `session.exited` carries a notice of kind `session.crashed`, severity `error`, setting `processCrashed` and no `suppressed`

#### Scenario: Pane closed by the user
- **WHEN** a pane is closed while a connection streams its session and the session's shell still runs
- **THEN** the `session.exited` for it has `userInitiated` true and its notice is suppressed `user-initiated`

#### Scenario: Waiting program that reports its status
- **WHEN** a session holds a `working` root record and its shell integration reports that a command finished
- **THEN** its `process.idle` carries a notice suppressed `program-status`

#### Scenario: Blocked program
- **WHEN** the root record of a session on the screen named `build` changes to `blocked` with the kind `permission` and the message `Run tests?`, and the change is marked `notify`
- **THEN** the event carries a notice of kind `program-status`, severity `warning`, setting `programStatus` and title `build: Waiting for permission: Run tests?`

#### Scenario: Unmarked status change
- **WHEN** a `program_status.changed` is not marked `notify`
- **THEN** it carries no notice

#### Scenario: Status of an unnamed screen
- **WHEN** a marked `blocked` change comes from a session on a screen without a name, second in the catalog
- **THEN** its notice's title starts with `Unnamed Screen: `, not with `Screen 2: `

### Requirement: Notifications in the Window
A window SHALL add a notification for an event with a notice that is not suppressed, while `notifications.enabled` and the switch the notice names are on in the window's settings, and SHALL take its kind, severity and title from the notice; a notification of a host's event SHALL be prefixed with the host's name. It SHALL add nothing for a suppressed notice. A notification for the pane the user watches (the focused pane of the shown screen, in a focused window) SHALL be recorded as read and show no notice in the window or from the system. Turning a switch off SHALL also remove the notifications the window holds that it governs, and turning it on again SHALL NOT bring them back.

#### Scenario: Own pane closed
- **WHEN** the user closes a pane whose shell still runs
- **THEN** the window adds no notification and shows no notice in the window or from the system

#### Scenario: Watched pane
- **WHEN** a notice that is not suppressed arrives for the pane the user watches
- **THEN** the window records it as read and shows no notice for it

#### Scenario: Switch off
- **WHEN** `notifications.processFinished` is off and a `process.finished` arrives for a pane the user is not watching
- **THEN** the window adds no notification

#### Scenario: Switch turned off later
- **WHEN** a window holds an unread `process.finished` notification and the user turns `notifications.processFinished` off
- **THEN** the notification goes and does not return when the switch is turned on again

### Requirement: Server-Chosen Screen Names
A server SHALL list `screen-names-v1` in `mux.welcome.capabilities`. A client SHALL send `create_screen`, and `move_pane_to_screen` with `createScreen`, for a new screen without a name to such a server, SHALL show the name the server reports for the screen, and SHALL send no label of its own in place of it. Until the name arrives it SHALL show a placeholder that names no screen. A screen the server reports without a name SHALL be shown as "Unnamed Screen", never by its position.

#### Scenario: New screen named by the server
- **WHEN** a window creates a screen on a server that announces `screen-names-v1` and holds "Screen 1" and "Screen 3"
- **THEN** its `create_screen` carries no `name`
- **AND** the window shows the screen as "Screen 2" once the server reports it

#### Scenario: Two windows create screens
- **WHEN** two windows each create a screen on the same server at the same time
- **THEN** the two screens get different names, chosen by the server in the order it applies the ops

#### Scenario: Screen without a name
- **WHEN** the server reports a screen with an empty name in second place, and another screen is named "Screen 2"
- **THEN** the window shows the unnamed one as "Unnamed Screen", not as "Screen 2"

#### Scenario: Older server
- **WHEN** a window creates a screen on a server whose `mux.welcome` lacks `screen-names-v1`
- **THEN** its `create_screen` names it "Screen N" with the smallest positive N that no screen of that server uses as far as the window knows when it sends the op

### Requirement: Review Texts And Anchors
A server that lists `viewer-reviews-v1` in `mux.welcome.capabilities` SHALL answer `viewer.anchor` with `viewer.anchored` and `viewer.review` with `viewer.reviewed`, on the same connection and to its sender only, or with `viewer.failed` under the same `requestId` (shapes in `docs/protocol.ts`). Both read the pane's annotations after the connection's earlier messages and its document as `viewer.read` does, change nothing, and name the document version they used.

#### Scenario: Anchors of a shown file
- **WHEN** a window sends `viewer.anchor` for viewer pane `v1`, which shows a file with a comment on line 3 and a general comment
- **THEN** it receives `viewer.anchored` with the file's version and, in the order of the pane's list, the general comment with the state `general` and the comment with the state `anchored`, its range in the source and lines 3 to 3

#### Scenario: Ranges in the source
- **WHEN** a pane's document starts with a byte order mark and has Windows line breaks
- **THEN** the ranges of `viewer.anchored` count UTF-8 bytes of the document without the byte order mark and with every line break as one line feed

#### Scenario: A review on request
- **WHEN** a window sends `viewer.review` for `v1` with the verdict `changes_requested`
- **THEN** it receives `viewer.reviewed` with the review's text, starting with the heading and `Verdict: changes requested`, the number of annotations it holds and the entries it names

#### Scenario: Passages of a rendered view
- **WHEN** a window sends `viewer.review` with the state and lines it found for an annotation taken from the rendered view, and for an annotation taken from the source
- **THEN** the review names the first with the window's lines and state and the second as the server anchored it

#### Scenario: A pane that is gone
- **WHEN** a window sends `viewer.review` for a pane the server does not have
- **THEN** it receives `viewer.failed` with the code `unknown_pane`, and nothing changes

#### Scenario: A malformed request
- **WHEN** a window sends `viewer.anchor` without a `paneId`, or `viewer.review` with an unknown verdict or a passage with a line of 0
- **THEN** it receives `viewer.failed` with the code `invalid`

#### Scenario: A passage of a removed annotation
- **WHEN** a window sends `viewer.review` with a passage of an annotation that another window removed just before
- **THEN** the server builds the review without that annotation and ignores the passage

#### Scenario: Through the hub
- **WHEN** a window shows a viewer pane of a host through the hub and asks for its review
- **THEN** the host builds the review, and the answer reaches the window over the same connection
