# Spec Delta

## MODIFIED Requirements

### Requirement: Two Control Transports
The server SHALL serve the control API on its local Unix control socket and, when explicitly started, through a browser gateway's loopback HTTP listener, and an endpoint that is available on both transports SHALL behave identically on both. Authorization on the HTTP listener follows `access-control`; the control socket accepts requests without any credential because access to it is restricted to the owning user. Where the socket lives, which endpoints it exposes and how it is protected are defined in `access-control` (Owner-Only Control Socket, Control Socket Route Set); a missing optional control socket SHALL not start an HTTP listener. Equivalent authorized native UI operations SHALL use the same service behavior without exposing additional control or grant routes.

#### Scenario: Same endpoint on both transports
- **WHEN** a client sends `GET /api/mux/status` over the control socket and another client sends it, authorized, over the HTTP listener
- **THEN** both receive status 200 with the same response content

#### Scenario: Control socket needs no credential
- **WHEN** a process of the owning user sends `GET /api/mux/status` over the control socket without any token or cookie
- **THEN** the server responds with status 200

### Requirement: Shared Workspace State
The control API SHALL read and modify the same workspace state that native and WebSocket clients use (see `mux-protocol` and `workspace-model`), so that a change made through either path is visible to the other immediately after it is applied.

#### Scenario: Op visible to a later query
- **WHEN** a client renames a screen through `POST /api/mux/op` and then sends `GET /api/mux/screens`
- **THEN** the screen list shows the new name

#### Scenario: UI change visible to the CLI
- **WHEN** a WebSocket client splits a pane and a CLI then sends `GET /api/mux/screen/{id}` for that screen
- **THEN** the returned screen contains the new pane

### Requirement: Server Status
`GET /api/mux/status` SHALL return a JSON object with `serverMode` (`daemon` or `embedded`, see `server-modes`), `version`, `screens` (number of screens), `sessions` (number of sessions across all screens), `activeScreen` (the active screen id, or `null` when there is none) and `listenAddr` (null for the listener-independent service), plus `instance` and `transportVersion`; any browser-gateway address is reported by that gateway's startup, not fabricated as a target-service listener.

#### Scenario: Status of a daemon
- **WHEN** a client sends `GET /api/mux/status` to a server running as a daemon with two screens and three sessions
- **THEN** the response has `serverMode` `daemon`, `screens` 2 and `sessions` 3

### Requirement: Build and Runtime Information
Through the browser gateway or the equivalent authorized native operation, `GET /api/build` SHALL return `{"version", "commit", "buildTime"}` for the running binary, with `commit` and `buildTime` set to `unknown` when the build did not record them, and, when the server runs inside WSL, `wslDistro` with the name of its distribution. `GET /api/stats` SHALL return the number of screens and the runtime backend.

#### Scenario: Build info
- **WHEN** an authorized client sends `GET /api/build`
- **THEN** the server responds with status 200 and the server version, commit and build time

#### Scenario: Build info inside WSL
- **WHEN** an authorized client sends `GET /api/build` to a server running in the WSL distribution `Ubuntu`
- **THEN** the response also carries `"wslDistro": "Ubuntu"`, and outside WSL it carries no `wslDistro`

### Requirement: Local Grant Administration Endpoints
Only the owner socket SHALL expose `POST /api/agents`, `GET /api/agents`, `GET /api/agents/{id}`, `POST /api/agents/{id}/revoke` and `POST /api/jobs/{id}/owner`. Creating a grant SHALL return its ID and socket path. Owner changes SHALL select a live grant or the user and obey `managed-jobs`. Grant sockets, native UI connections and authenticated TCP callers SHALL receive 404 for these paths; TCP requests rejected earlier by the existing guards/authentication SHALL retain those responses.

#### Scenario: Administrative transfer
- **WHEN** the owner socket receives a valid request to move a job to a new grant
- **THEN** the same process and output become available to that grant and inaccessible to the previous grant

#### Scenario: Administration attempted through TCP
- **WHEN** an authenticated TCP client requests grant creation, enumeration, revocation or job ownership changes
- **THEN** it receives 404 and no grant or job ownership changes
