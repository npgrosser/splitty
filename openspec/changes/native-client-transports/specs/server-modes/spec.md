# Spec Delta

## MODIFIED Requirements

### Requirement: Two Server Modes
The server SHALL run in exactly one of two modes for its whole lifetime: `daemon`, started by `splitty daemon start`, or `embedded`, used by plain `splitty serve` and by the backends the desktop app starts; a `serve --daemon` or remote browser gateway is only a client of its target service's mode. Persistence SHALL be a property of the mode, not of individual screens: a daemon persists and restores its workspace, and an embedded server persists nothing.

#### Scenario: Serve runs embedded
- **WHEN** a user runs `splitty serve`
- **THEN** the server runs in `embedded` mode

#### Scenario: Daemon start runs as daemon
- **WHEN** a user runs `splitty daemon start`
- **THEN** the server runs in `daemon` mode

### Requirement: Ephemeral Port And Bind Before Detach
An explicit browser gateway using port 0 SHALL report the actual OS-assigned loopback port everywhere, including its origin and startup output. A daemon SHALL bind its mandatory private client endpoint and acquire its instance lock before detaching. Failure SHALL be reported in the foreground with no orphan daemon. Native daemon discovery SHALL not invent or require a TCP port.

#### Scenario: OS-assigned port reported
- **WHEN** `splitty serve --addr 127.0.0.1:0` starts
- **THEN** its startup output and origin use the same actual non-zero bound port

#### Scenario: Endpoint unavailable
- **WHEN** a daemon cannot securely bind its private client endpoint
- **THEN** startup fails in the foreground without detaching or opening a TCP listener

#### Scenario: Port already in use
- **WHEN** an explicit browser gateway is asked to bind a port already used by another process
- **THEN** it fails in the foreground without leaving a gateway or owned embedded service running

### Requirement: Desktop Stdio Lifetime Binding
A child started with `desktop-backend --transport native-v1` SHALL select `--backend embedded|ssh`: an owned embedded service or the shared SSH flow with a validated `--target`. It SHALL use inherited stdin/stdout only for its negotiated connection, with bounded diagnostics on stderr. EOF SHALL stop owned embedded sessions or only detach from an SSH host. Protocol bytes SHALL not be decoded as text. The former `serve --desktop-stdio` SHALL be refused with migration guidance.

#### Scenario: Launcher exits
- **WHEN** the launcher closes the child's input pipe
- **THEN** the child stops accepting work and shuts down its embedded sessions

#### Scenario: Binary roundtrip
- **WHEN** terminal output includes NULs, non-UTF-8 bytes or split frame headers
- **THEN** those bytes are delivered unchanged and no log or bootstrap line contaminates stdout

#### Scenario: Legacy launcher
- **WHEN** a caller requests `serve --desktop-stdio`
- **THEN** it gets a usage error pointing to the native child entry point, without opening a listener

#### Scenario: Launcher requests a fresh code
- **WHEN** a launcher writes the old `bootstrap` line to the native child's input
- **THEN** negotiation fails as invalid input without printing a bootstrap line or opening HTTP

#### Scenario: Helper attached to an independent SSH daemon
- **WHEN** a native helper connected to an SSH daemon loses its launcher
- **THEN** it closes its connection and owned SSH process without stopping the daemon or its sessions

### Requirement: Embedded Mode Lifetime
An embedded server SHALL keep its workspace and sessions only in memory, for the lifetime of its process. It SHALL NOT read a persisted workspace at startup, SHALL NOT write one, and SHALL NOT register itself in `daemon.json`. When `splitty serve` receives an interrupt (Ctrl+C), or a native child backend reaches end-of-file on its inherited input, it SHALL close all sessions and stop.

#### Scenario: Serve starts empty
- **WHEN** `splitty serve` starts while a `state.json` from a daemon exists in the configuration directory
- **THEN** the embedded server starts with an empty workspace and leaves `state.json` untouched

#### Scenario: Interrupt ends sessions
- **WHEN** `splitty serve` receives Ctrl+C
- **THEN** every terminal session it hosts is closed before the process exits

### Requirement: Desktop App Hosting On macOS And Linux
On macOS and Linux the desktop app SHALL always start an owned embedded local service without a UI TCP listener. Its local workspace and jobs SHALL end when the app exits. The app SHALL NOT automatically connect to, start, restart or stop an independent local daemon, adopt its keepers or modify its discovery/workspace files. Existing, incompatible, unreachable, stale or corrupt daemon discovery SHALL not change desktop startup. Persistent local access remains explicit through the CLI or `serve --daemon`. Popouts SHALL share the owning app's service lifetime.

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
- **WHEN** an independent local daemon has no usable bootstrap endpoint
- **THEN** the desktop app still starts its own embedded service over IPC without reading a browser token or requesting a code from the daemon

#### Scenario: Stale daemon file
- **WHEN** local daemon discovery is stale or corrupt
- **THEN** the app starts embedded and leaves the independent daemon files untouched

#### Scenario: Kept sessions after a daemon crash
- **WHEN** keepers remain after a daemon crash and the app starts
- **THEN** the app starts its own embedded workspace without adopting, restarting or ending those kept sessions

#### Scenario: App exit closes only its local sessions
- **WHEN** the desktop app exits while an independent local daemon and a remote daemon have sessions
- **THEN** its embedded local sessions and jobs end, while the independent daemon and remote sessions keep running

### Requirement: Desktop App Hosting On Windows
The native Windows app SHALL run its local embedded backend in the selected supported WSL 2 distribution through inherited pipes using `desktop-backend --transport native-v1`. It SHALL need no localhost forwarding, discover no local daemon, and reuse its live backend for that distribution. Switching distributions SHALL revoke old client handles and stop the previous backend. On exit it SHALL close input, wait up to 3 seconds, then terminate only its owned bridge if needed, never the distribution. Remote-only windows SHALL work without WSL.

#### Scenario: App exit stops the backend
- **WHEN** the Windows app quits
- **THEN** pipe EOF shuts down its embedded sessions without requiring a TCP connection

#### Scenario: Backend does not stop in time
- **WHEN** the owned backend does not stop within 3 seconds
- **THEN** the app terminates its bridge only and leaves unrelated WSL work running

#### Scenario: Repeated start for the same distribution
- **WHEN** the app requests the already live selected distribution
- **THEN** no second backend or listener is started

#### Scenario: Remote only
- **WHEN** Windows has no WSL and the user opens a host
- **THEN** the packaged remote window uses native SSH without starting a local backend

#### Scenario: WSL localhost forwarding unavailable
- **WHEN** WSL localhost forwarding is disabled or unavailable
- **THEN** the local desktop's terminal, API and file traffic still works through the owned pipes

### Requirement: Daemon Detachment
`splitty daemon start` SHALL detach the daemon from the invoking terminal and session so that it keeps running after the terminal closes. The invoking command SHALL return only once the daemon is serving: its private client endpoint accepts and negotiates connections, its control socket is bound or was deliberately skipped, and format-2 `daemon.json` is written. If the daemon fails before that point, the invoking command SHALL report the failure (see `cli`) and SHALL leave no `daemon.json` that names the failed process. The daemon SHALL append its log output to `daemon.log` in the Splitty configuration directory.

#### Scenario: Terminal closed after start
- **WHEN** a user runs `splitty daemon start` and then closes the terminal it was run from
- **THEN** the daemon keeps running and serving clients

#### Scenario: Ready when the command returns
- **WHEN** `splitty daemon start` exits with status 0
- **THEN** a control command such as `splitty list-screens` run right afterwards reaches the daemon

#### Scenario: Daemon fails during startup
- **WHEN** the daemon process fails after detaching but before it is serving
- **THEN** `splitty daemon start` exits with a non-zero status
- **AND** `splitty daemon status` reports that no daemon is running

### Requirement: Daemon Info File
A running daemon SHALL atomically publish owner-only `daemon.json` with `format:2`, `pid`, `clientSocket`, `socket` (the control socket or empty when unavailable), `instance`, `transportVersion:1`, `startedAt`, `version`, mux `protocol` and `keepsSessions:true` in its configuration directory. It SHALL contain no credential or required TCP port. Publication occurs only after readiness. Missing/unparsable records mean no discovery; a dead process is stale. Legacy records SHALL be recognized for diagnostics without automatically connecting to their port.

#### Scenario: Running daemon advertised
- **WHEN** a new daemon publishes discovery
- **THEN** its client socket and instance can be verified, its permissions are 0600 and no port or credential is advertised

#### Scenario: Legacy record
- **WHEN** discovery finds a live legacy daemon described by a port
- **THEN** the client reports transport compatibility/update guidance without treating the port as a native endpoint

#### Scenario: Control socket unavailable
- **WHEN** the optional control socket could not be created but the client endpoint is ready
- **THEN** discovery contains an empty control socket and the verified private client endpoint

#### Scenario: Corrupt info file
- **WHEN** `daemon.json` contains invalid JSON
- **THEN** `splitty daemon status` reports `No daemon running`
- **AND** a subsequent start still checks the instance lock and private endpoint before creating a workspace

### Requirement: Daemon Graceful Shutdown
On SIGTERM or SIGINT, the daemon SHALL stop accepting connections, release the keepers of its sessions without ending the sessions (see `session-keepers`), write the final workspace state to `state.json`, remove `daemon.json`, and remove its control and client sockets and release its instance lock before exiting.

#### Scenario: Graceful stop
- **WHEN** the daemon receives SIGTERM
- **THEN** `state.json` reflects the workspace as of shutdown
- **AND** `daemon.json` no longer exists after the process has exited

#### Scenario: Sessions keep running
- **WHEN** the daemon receives SIGTERM while a pane runs a build
- **THEN** the build keeps running after the daemon has exited

### Requirement: Single Daemon Instance
At most one daemon SHALL be registered per user configuration directory. If `daemon.json` names a live process, `splitty daemon start` SHALL report the running daemon (see `cli`) and exit successfully without starting another daemon. If the named process is not alive, the stale file SHALL be removed and the start SHALL proceed. Embedded servers are not subject to this limit. Daemon mode SHALL be refused with a non-zero exit status on platforms other than Unix.

#### Scenario: Daemon already running
- **WHEN** `splitty daemon start` is run while the daemon recorded in `daemon.json` is alive
- **THEN** it reports the running daemon's pid and private endpoint (or explicit legacy status) and exits with status 0
- **AND** no second daemon process exists

#### Scenario: Stale registration
- **WHEN** `splitty daemon start` finds a `daemon.json` whose pid is not alive
- **THEN** it removes the file and starts a new daemon

## ADDED Requirements

### Requirement: Browser Adapter Lifetime
A browser gateway SHALL own its listener and browser credentials independently of the target service. Stopping a gateway attached to a daemon SHALL only detach; stopping plain `serve` SHALL also stop its owned embedded service. A dropped target connection SHALL not release the browser listener or select another backend.

#### Scenario: Daemon browser closed
- **WHEN** `splitty serve --daemon` ends
- **THEN** its listener closes and the daemon and its sessions keep running

#### Scenario: Target temporarily unavailable
- **WHEN** a gateway loses its daemon or SSH connection
- **THEN** its listener stays bound and target operations fail or wait only within their documented deadlines until reconnect
