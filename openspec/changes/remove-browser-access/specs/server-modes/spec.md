## ADDED Requirements

### Requirement: Daemon And Embedded Modes
The server SHALL run in exactly one of two modes for its whole lifetime: `daemon`, started by `splitty daemon start`, or `embedded`, used by the local service the desktop app starts. Persistence SHALL be a property of the mode, not of individual screens: a daemon persists and restores its workspace, and an embedded server persists nothing.

#### Scenario: Desktop backend runs embedded
- **WHEN** the desktop app starts its local service with `desktop-backend --transport native-v1 --backend embedded`
- **THEN** the server runs in `embedded` mode

#### Scenario: Daemon start runs as daemon
- **WHEN** a user runs `splitty daemon start`
- **THEN** the server runs in `daemon` mode

### Requirement: Embedded Service Lifetime
An embedded server SHALL keep its workspace and sessions only in memory, for the lifetime of its process. It SHALL NOT read a persisted workspace at startup, SHALL NOT write one, and SHALL NOT register itself in `daemon.json`. When its native child backend reaches end-of-file on its inherited input, or the desktop app that owns it in-process exits, it SHALL close all sessions and stop.

#### Scenario: Embedded service starts empty
- **WHEN** the desktop app's embedded service starts while a `state.json` from a daemon exists in the configuration directory
- **THEN** the embedded server starts with an empty workspace and leaves `state.json` untouched

#### Scenario: Closed input ends sessions
- **WHEN** the launcher of `desktop-backend --transport native-v1 --backend embedded` closes the child's input
- **THEN** every terminal session it hosts is closed before the process exits

### Requirement: Daemon Binds Before Detach
The daemon launcher SHALL acquire the instance lock before starting its detached child and SHALL wait for that child to bind the mandatory private client endpoint and complete native readiness before reporting success. Failure SHALL be reported in the foreground with no orphan daemon. Native daemon discovery SHALL not invent or require a TCP port.

#### Scenario: Endpoint unavailable
- **WHEN** a daemon cannot securely bind its private client endpoint
- **THEN** startup fails in the foreground without detaching or opening a TCP listener

#### Scenario: Ready before success
- **WHEN** `splitty daemon start` reports `SPLITTY_CLIENT_SOCKET=<path>`
- **THEN** a native client can negotiate with the daemon at that path at once

## MODIFIED Requirements

### Requirement: Server Mode Reporting
The server SHALL report its mode to clients as `serverMode` with the value `"daemon"` or `"embedded"` in every `mux.welcome` message (see `mux-protocol`) and in the response of `GET /api/mux/status` (see `control-api`).

#### Scenario: Welcome from a daemon
- **WHEN** a client connects to a server started with `splitty daemon start`
- **THEN** the `mux.welcome` it receives carries `serverMode: "daemon"`

#### Scenario: Status from an embedded server
- **WHEN** a pane process of the desktop app's embedded service requests `GET /api/mux/status` through `SPLITTY_SOCK`
- **THEN** the response contains `"serverMode": "embedded"`

### Requirement: Desktop App Hosting On macOS And Linux
On macOS and Linux the desktop app SHALL always start an owned embedded local service without a UI TCP listener. Its local workspace and jobs SHALL end when the app exits. The app SHALL NOT automatically connect to, start, restart or stop an independent local daemon, adopt its keepers or modify its discovery/workspace files. Existing, incompatible, unreachable, stale or corrupt daemon discovery SHALL not change desktop startup. Persistent local access remains explicit through `splitty daemon start`.

#### Scenario: Daemon available
- **WHEN** a compatible local daemon is already running
- **THEN** the app starts its own empty embedded service and leaves the daemon and its sessions untouched

#### Scenario: No daemon
- **WHEN** there is no daemon and no kept session
- **THEN** the app starts its embedded service without opening a UI TCP listener

#### Scenario: Independent daemon cannot admit clients
- **WHEN** a local daemon is incompatible or unreachable
- **THEN** normal embedded desktop startup still works without connecting to or updating that daemon

#### Scenario: Daemon does not issue a code
- **WHEN** an independent local daemon runs
- **THEN** the desktop app starts its own embedded service in-process without requesting anything from the daemon or reading a credential file

#### Scenario: Stale daemon file
- **WHEN** local daemon discovery is stale or corrupt
- **THEN** the app starts embedded and leaves the independent daemon files untouched

#### Scenario: Kept sessions after a daemon crash
- **WHEN** keepers remain after a daemon crash and the app starts
- **THEN** the app starts its own embedded workspace without adopting, restarting or ending those kept sessions

#### Scenario: App exit closes only its local sessions
- **WHEN** the desktop app exits while an independent local daemon and a remote daemon have sessions
- **THEN** its embedded local sessions and jobs end, while the independent daemon and remote sessions keep running

### Requirement: Configuration Directory Override
When the environment variable `SPLITTY_CONFIG_DIR` holds an absolute path, a Splitty server, daemon or CLI SHALL use that directory instead of the platform's Splitty configuration directory for every file kept there, among them `config.toml`, `daemon.json`, `daemon.log`, `state.json`, templates and the known hosts. A relative or empty value SHALL be ignored. The directory SHALL be created owner-only like the default one.

#### Scenario: Daemon with its own directory
- **WHEN** a daemon is started with `SPLITTY_CONFIG_DIR=/home/me/.config/splitty-b`
- **THEN** it writes `daemon.json` and `daemon.log` in `/home/me/.config/splitty-b`, and `splitty daemon status` with the same variable finds it while one without it does not

#### Scenario: Relative value
- **WHEN** `SPLITTY_CONFIG_DIR` is `relative/dir`
- **THEN** the platform's configuration directory is used

## REMOVED Requirements

### Requirement: Ephemeral Port And Bind Before Detach
**Reason**: Its port half described the browser gateway's port 0 and port-in-use handling, which go with the gateway. The daemon half is unchanged and continues as Daemon Binds Before Detach.

### Requirement: Browser Adapter Lifetime
**Reason**: The browser gateway it described, plain `serve` and `serve --daemon` are removed. The embedded service's lifetime is Embedded Mode Lifetime and Desktop Backend Child Lifetime; a daemon's is the daemon requirements.

**Migration**: Use `splitty daemon start` for a persistent service and the desktop app or `splitty attach` as its clients.

### Requirement: Two Server Modes
**Reason**: Restated as Daemon And Embedded Modes without plain `serve` and `serve --daemon`, which are removed; the two modes are unchanged.

### Requirement: Embedded Mode Lifetime
**Reason**: Restated as Embedded Service Lifetime without `splitty serve` and its interrupt, which are removed; the in-memory lifetime is unchanged.
