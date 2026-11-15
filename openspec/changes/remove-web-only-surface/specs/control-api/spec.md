## MODIFIED Requirements

### Requirement: HTTP Form Of Native Operations
Requirements that name an operation by an HTTP method and an `/api/...` path name its HTTP form in the operation catalog, which the native operation of the same catalog entry shares (validation, status and error object). No listener SHALL serve that form: the owner control socket routes only the paths of Control Socket Route Set (see `access-control`) and answers 404 for every other one, and every other operation is reached only as its native operation over an authorized client connection, within the role that connection has (see `client-connections`, Enumerated Native Requests).

#### Scenario: Native operation with an HTTP name
- **WHEN** a native main client requests `remote.directory.get`, which the catalog also names `GET /api/remote/directory`
- **THEN** it receives the merged host directory

#### Scenario: The same path on the control socket
- **WHEN** a pane process sends `GET /api/remote/directory` or `GET /api/home/settings` over the owner control socket
- **THEN** the server answers 404 and changes nothing

### Requirement: Server Status
`GET /api/mux/status` SHALL return a JSON object with `serverMode` (`daemon` or `embedded`, see `server-modes`), `version`, `screens` (number of screens), `sessions` (number of sessions across all screens) and `activeScreen` (the active screen id, or `null` when there is none), plus `instance` and `transportVersion`. It SHALL carry no listener address, since the service has no TCP listener.

#### Scenario: Status of a daemon
- **WHEN** a client sends `GET /api/mux/status` to a server running as a daemon with two screens and three sessions
- **THEN** the response has `serverMode` `daemon`, `screens` 2 and `sessions` 3

#### Scenario: No listener address
- **WHEN** a client sends `GET /api/mux/status` to a daemon or an embedded service
- **THEN** the response has no `listenAddr`
