# Spec Delta

## MODIFIED Requirements

### Requirement: Welcome on Connect
Immediately after the upgrade, without waiting for any client message, the server SHALL send `mux.welcome` containing the connection's `clientId`, the list of screens in workspace order (each with `id` and optional `name`), and `serverMode` set to `embedded` or `daemon` (see `server-modes`). It SHALL additionally advertise `session-control-v1` in its capabilities; clients MUST NOT infer support solely from the server version.

#### Scenario: Welcome lists existing screens
- **WHEN** a client connects to a server whose workspace holds screens `C`, `A`, `B` in that order
- **THEN** `mux.welcome.screens` SHALL list `C`, `A`, `B` in that order

#### Scenario: Empty workspace
- **WHEN** a client connects to a server with no screens
- **THEN** `mux.welcome.screens` SHALL be an empty list

#### Scenario: Control capability advertised
- **WHEN** a client connects to a server enforcing session control
- **THEN** its welcome advertises `session-control-v1` before any terminal mutation is accepted


### Requirement: Disconnect Handling
When a connection ends through a close frame, a transport error, or heartbeat timeout, the server SHALL stop delivering events and terminal output to it. A disconnect MUST NOT terminate any session and MUST NOT change workspace state, and no general client-disconnect notification SHALL be sent. The server SHALL release session controls held by that connection and notify the affected session subscribers through `control.state`, as defined in `terminal-sessions`.

#### Scenario: Client closes the window
- **WHEN** the only connected client disconnects
- **THEN** all sessions SHALL keep running
- **AND** a client that connects later SHALL see the same screens in `mux.welcome`

#### Scenario: Controller disconnects
- **WHEN** a controlling connection ends by close, transport error or heartbeat timeout
- **THEN** its session controls are released with new generations and remaining observers receive the new state
- **AND** sessions, layouts and the last PTY dimensions are retained
