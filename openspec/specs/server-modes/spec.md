# server-modes Specification

## Purpose
Defines how the Splitty server runs and what outlives what: the embedded and daemon server modes, how desktop launchers bind the server's lifetime to theirs, how a running daemon advertises itself, and which workspace state survives a daemon restart.

## Requirements

### Requirement: Two Server Modes
The server SHALL run in exactly one of two modes for its whole lifetime: `daemon`, started by `splitty daemon start`, or `embedded`, used by plain `splitty serve` and by the backends the desktop app starts; a `serve --daemon` or remote browser gateway is only a client of its target service's mode. Persistence SHALL be a property of the mode, not of individual screens: a daemon persists and restores its workspace, and an embedded server persists nothing.

#### Scenario: Serve runs embedded
- **WHEN** a user runs `splitty serve`
- **THEN** the server runs in `embedded` mode

#### Scenario: Daemon start runs as daemon
- **WHEN** a user runs `splitty daemon start`
- **THEN** the server runs in `daemon` mode

### Requirement: Server Mode Reporting
The server SHALL report its mode to clients as `serverMode` with the value `"daemon"` or `"embedded"` in every `mux.welcome` message (see `mux-protocol`) and in the response of `GET /api/mux/status` (see `control-api`).

#### Scenario: Welcome from a daemon
- **WHEN** a client connects to a server started with `splitty daemon start`
- **THEN** the `mux.welcome` it receives carries `serverMode: "daemon"`

#### Scenario: Status from an embedded server
- **WHEN** a caller requests `GET /api/mux/status` from a server started with `splitty serve`
- **THEN** the response contains `"serverMode": "embedded"`

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

### Requirement: Desktop App Command Search Path
On macOS and Linux, a desktop app whose standard input is not a terminal SHALL, before it starts its backend or a remote window, ask the user's login shell for its `PATH`: the shell named by `SHELL`, started as an interactive login shell (csh and tcsh as interactive shells), without input. The app SHALL then use the login shell's `PATH` entries, followed by the inherited `PATH` entries the login shell lacks, for every program it starts, among them `ssh` and with it the `ProxyCommand` of the user's SSH configuration. When `SHELL` is unset or not an absolute path, when the shell fails, or when it reports no `PATH` within 5 seconds, the app SHALL keep its inherited `PATH`; a program the shell's startup leaves running SHALL NOT delay the app beyond the shell's answer. The app SHALL take no variable of the shell's environment other than `PATH`. A desktop app whose standard input is a terminal SHALL keep the `PATH` it inherited.

#### Scenario: Started from the Dock
- **WHEN** macOS starts the app with the `PATH` `/usr/bin:/bin:/usr/sbin:/sbin`, and the user's zsh startup adds `/opt/homebrew/bin`
- **THEN** the app's `PATH` starts with the login shell's entries, `/opt/homebrew/bin` among them, and still contains `/usr/sbin` and `/sbin`
- **AND** a remote window to a host whose `ProxyCommand` runs `gcloud` from `/opt/homebrew/bin` reaches the host

#### Scenario: Shell startup does not answer
- **WHEN** the login shell has not reported a `PATH` after 5 seconds, or `SHELL` is not an absolute path
- **THEN** the app starts with its inherited `PATH`

#### Scenario: Background program of the startup files
- **WHEN** the shell's startup files leave a program running in the background that holds the shell's output open
- **THEN** the app uses the `PATH` the shell reported, without waiting for that program

#### Scenario: Started from a terminal
- **WHEN** the app is started from a terminal
- **THEN** it keeps the terminal's `PATH` and does not start a login shell

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

### Requirement: Single Daemon Instance
At most one daemon SHALL be registered per user configuration directory. If `daemon.json` names a live process, `splitty daemon start` SHALL report the running daemon (see `cli`) and exit successfully without starting another daemon. If the named process is not alive, cleanup SHALL acquire the native instance lock and reread the unchanged record before removing it; startup SHALL proceed only after acquiring the lock and securely binding the endpoint. A held lock SHALL prevent replacement of a live or starting instance even before discovery is published. Embedded servers are not subject to this limit. Daemon mode SHALL be refused with a non-zero exit status on platforms other than Unix.

#### Scenario: Daemon already running
- **WHEN** `splitty daemon start` is run while the daemon recorded in `daemon.json` is alive
- **THEN** it reports the running daemon's pid and private endpoint (or explicit legacy status) and exits with status 0
- **AND** no second daemon process exists

#### Scenario: Stale registration
- **WHEN** `splitty daemon start` finds a `daemon.json` whose pid is not alive
- **THEN** it removes the unchanged stale file only after acquiring the native instance lock and starts a new daemon only when the private endpoint can be securely bound

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
A running daemon SHALL atomically publish owner-only `daemon.json` with `format:2`, `pid`, `clientSocket`, `socket` (the control socket or empty when unavailable), `instance`, `transportVersion:1`, `startedAt`, `version`, mux `protocol` and `keepsSessions:true` in its configuration directory. It SHALL contain no credential or required TCP port. Publication occurs only after readiness. A missing record means no published discovery; corrupt or unreadable discovery SHALL fail explicitly without inferring absence or deleting the record. A dead process is stale, subject to the instance-lock and unchanged-record checks before cleanup. Legacy records SHALL be recognized for diagnostics without automatically connecting to their port.

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
- **THEN** `splitty daemon status`, including `--json`, exits nonzero with a diagnostic and leaves the record unchanged
- **AND** neither status nor startup treats corrupt discovery as permission to create another workspace

### Requirement: Daemon Graceful Shutdown
On SIGTERM or SIGINT, the daemon SHALL stop accepting connections, release the keepers of its sessions without ending the sessions (see `session-keepers`), write the final workspace state to `state.json`, remove `daemon.json`, and remove its control and client sockets and release its instance lock before exiting.

#### Scenario: Graceful stop
- **WHEN** the daemon receives SIGTERM
- **THEN** `state.json` reflects the workspace as of shutdown
- **AND** `daemon.json` no longer exists after the process has exited

#### Scenario: Sessions keep running
- **WHEN** the daemon receives SIGTERM while a pane runs a build
- **THEN** the build keeps running after the daemon has exited

### Requirement: Workspace Persistence In Daemon Mode
A daemon SHALL persist the workspace to `state.json` in the Splitty configuration directory after every successfully applied workspace change, replacing the file atomically so that a reader never sees a partially written file. A rejected change SHALL NOT trigger a write. A failed write SHALL be logged and SHALL NOT stop the daemon. The persisted workspace SHALL include the screens with their names, order, and active screen, each screen's layout including floating windows, the panes, and for each session the record needed to start it again (command, working directory, shell or shell profile, status) and its scratchpad (see `session-scratchpad`). It SHALL additionally include managed-job identity, launch metadata, owner/creator identifiers, lifecycle and deduplication records, with an explicit never-auto-respawn marker on job sessions. It SHALL NOT include terminal output or scrollback, running process state, client connections, grant socket paths or live grant authority. File permissions are governed by `access-control`.

#### Scenario: Layout change persisted
- **WHEN** a client splits a pane on a daemon
- **THEN** `state.json` contains the new pane and its session record once the change has been applied

#### Scenario: Rejected change
- **WHEN** a workspace operation is rejected as invalid
- **THEN** `state.json` is not rewritten

#### Scenario: Abrupt termination
- **WHEN** the daemon process is killed without a chance to shut down
- **THEN** `state.json` still holds the workspace as of the last successfully applied change

#### Scenario: Job lifecycle persisted without a reader
- **WHEN** a managed job exits without an attached UI
- **THEN** its terminal result is persisted without waiting for a layout change, and no output or socket authority is written

#### Scenario: Scratchpad persisted
- **WHEN** a client saves text in a session's scratchpad on a daemon
- **THEN** `state.json` holds that text in the session's record once the change has been applied

### Requirement: Workspace Restore On Daemon Start
When a daemon starts and a readable `state.json` exists, it SHALL restore that workspace, keeping the screen, pane, and session identifiers. If the file is missing or cannot be parsed, the daemon SHALL start with an empty workspace.

#### Scenario: Restart restores layout
- **WHEN** a daemon is stopped and started again
- **THEN** clients see the same screens, names, order, and layouts with the same identifiers as before the stop

#### Scenario: Corrupt state file
- **WHEN** `state.json` contains invalid JSON at daemon start
- **THEN** the daemon starts with an empty workspace

### Requirement: Session Respawn On Daemon Start
When a daemon restores a workspace, it SHALL start a new process for every ordinary pane that is bound to a session not recorded as exited and whose keeper the daemon has not adopted (see `session-keepers`). The new process SHALL keep that session's identifier, command, and shell or shell profile, and SHALL start in the session's recorded working directory, or the pane's if the session has none. Sessions recorded with neither a shell nor a shell profile SHALL use the current default shell profile. Output from before the restart SHALL NOT be restored for a respawned session. A session that fails to start SHALL be logged and SHALL NOT prevent the daemon from serving or the other sessions from starting. Managed-job sessions SHALL never be respawned, including when their recorded state was `starting` or `running`. An unfinished job whose keeper the daemon does not adopt SHALL become `interrupted` with an unknown exit result and SHALL report its old output unavailable; a job whose keeper it adopts SHALL keep running (see Grant And Client Lifetimes Are Independent). Terminal jobs SHALL retain their recorded outcome.

#### Scenario: Shells respawned with the same identifiers
- **WHEN** a daemon whose workspace had two ordinary terminal panes, whose keepers are gone, restarts
- **THEN** each pane is bound to a running session with its previous session identifier
- **AND** each session's terminal starts without the previous output

#### Scenario: Adopted shell is not respawned
- **WHEN** a daemon restarts while the keeper of a pane's shell is alive
- **THEN** the pane is bound to the adopted session, and no second shell is started for it

#### Scenario: One session fails to respawn
- **WHEN** one recorded ordinary session's command cannot be started during restore
- **THEN** the remaining sessions are started and the daemon accepts clients

#### Scenario: Backend job is not replayed
- **WHEN** a daemon restarts with a previously running backend job whose keeper is gone and an ordinary shell
- **THEN** only the ordinary shell is respawned, the job remains visible as interrupted, and neither a job process nor a replacement shell starts in its pane

### Requirement: Session Lifetime Across Modes
In both modes, sessions SHALL outlive client connections: a client disconnecting SHALL NOT end any session (see `terminal-sessions` and `mux-protocol`). Sessions SHALL end with the server process in embedded mode. In daemon mode, sessions SHALL outlive the daemon process in their keepers (see `session-keepers`); they end when they are closed, when their process exits, or when `splitty daemon stop --end-sessions` ends them (see `cli`). A session whose keeper is gone continues only through the respawn defined above.

#### Scenario: Window closed while connected to a daemon
- **WHEN** the only client of a daemon disconnects and later reconnects
- **THEN** the same sessions are still running and are offered to the reconnecting client

#### Scenario: Embedded server stops
- **WHEN** an embedded server shuts down
- **THEN** none of its sessions are available to any later server

#### Scenario: Daemon restarted
- **WHEN** a daemon is stopped and started again while its panes run shells
- **THEN** the reconnecting client is offered the same, still running sessions

### Requirement: Grant And Client Lifetimes Are Independent
Live agent grants SHALL belong to the server process, not to an invoking CLI process or connection. Client exit SHALL neither revoke a grant nor stop its jobs. Server shutdown SHALL invalidate all grant sockets and wake waiting requests. An embedded server's shutdown SHALL stop its managed processes and retain no job metadata for its next start; a daemon's shutdown SHALL leave job processes running in their keepers. Daemon restoration SHALL not recreate grant authority from owner labels: restored and adopted jobs are under trusted administration.

#### Scenario: Client and server restart differ
- **WHEN** an agent client restarts while the server remains alive
- **THEN** its grant and jobs remain usable
- **AND** after the server itself restarts the old grant no longer works and trusted administration must explicitly grant access to any restored records

#### Scenario: Job outlives a daemon restart
- **WHEN** a daemon restarts while an agent's dev server job runs
- **THEN** the job keeps running with its process, the agent's old grant no longer works, and the owner can inspect, read and stop the job

### Requirement: Legacy Workspaces Remain Readable
Workspace state without managed-job fields SHALL load with no managed jobs and retain ordinary session behavior. A job record lacking a runnable session SHALL never be interpreted as an instruction to create an ordinary shell. Malformed job additions SHALL fail safely rather than cause command replay.

#### Scenario: Upgrade from an older state file
- **WHEN** the server loads a valid state file written before managed jobs existed
- **THEN** its ordinary screens and shells restore as before and no managed job is inferred

### Requirement: Configuration Directory Override
When the environment variable `SPLITTY_CONFIG_DIR` holds an absolute path, a Splitty server, daemon or CLI SHALL use that directory instead of the platform's Splitty configuration directory for every file kept there, among them `config.toml`, `auth-token`, `daemon.json`, `daemon.log`, `state.json`, templates and the known hosts. A relative or empty value SHALL be ignored. The directory SHALL be created owner-only like the default one.

#### Scenario: Daemon with its own directory
- **WHEN** a daemon is started with `SPLITTY_CONFIG_DIR=/home/me/.config/splitty-b`
- **THEN** it writes `daemon.json` and reads `auth-token` in `/home/me/.config/splitty-b`, and `splitty daemon status` with the same variable finds it while one without it does not

#### Scenario: Relative value
- **WHEN** `SPLITTY_CONFIG_DIR` is `relative/dir`
- **THEN** the platform's configuration directory is used

### Requirement: Browser Adapter Lifetime
A browser gateway SHALL own its listener and browser credentials independently of the target service. Stopping a gateway attached to a daemon SHALL only detach; stopping plain `serve` SHALL also stop its owned embedded service. A dropped target connection SHALL not release the browser listener or select another backend.

#### Scenario: Daemon browser closed
- **WHEN** `splitty serve --daemon` ends
- **THEN** its listener closes and the daemon and its sessions keep running

#### Scenario: Target temporarily unavailable
- **WHEN** a gateway loses its daemon or SSH connection
- **THEN** its listener stays bound and target operations fail or wait only within their documented deadlines until reconnect
