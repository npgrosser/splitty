# Spec Delta

## MODIFIED Requirements

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

### Requirement: Welcome on Connect
Immediately after an authorized WebSocket upgrade or completed native service negotiation, without waiting for any client message, the server SHALL send `mux.welcome` containing the connection's `clientId`, the list of screens in workspace order (each with `id` and optional `name`), and `serverMode` set to `embedded` or `daemon` (see `server-modes`).

#### Scenario: Welcome lists existing screens
- **WHEN** a client connects to a server whose workspace holds screens `C`, `A`, `B` in that order
- **THEN** `mux.welcome.screens` SHALL list `C`, `A`, `B` in that order

#### Scenario: Empty workspace
- **WHEN** a client connects to a server with no screens
- **THEN** `mux.welcome.screens` SHALL be an empty list

### Requirement: Heartbeat and Liveness
The server SHALL send a ping every 15 seconds, using WebSocket control frames for browser connections and transport control messages for native connections and SHALL close a connection when, at a heartbeat check, no valid frame of any kind has been received from the client for more than 30 seconds.

#### Scenario: Responsive client
- **WHEN** a client answers pings with pongs
- **THEN** the server SHALL keep the connection open

#### Scenario: Silent client
- **WHEN** a client sends no frame, including pongs, for more than 30 seconds
- **THEN** the server SHALL close the connection at the next heartbeat check

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
