# server-modes Specification

## Purpose
Defines how the Splitty server runs and what outlives what: the embedded and daemon server modes, how desktop launchers bind the server's lifetime to theirs, how a running daemon advertises itself, and which workspace state survives a daemon restart.

## Requirements

### Requirement: Two Server Modes
The server SHALL run in exactly one of two modes for its whole lifetime: `daemon`, started by `splitty daemon start`, or `embedded`, used by `splitty serve` and by the backends the desktop app starts. Persistence SHALL be a property of the mode, not of individual screens: a daemon persists and restores its workspace, and an embedded server persists nothing.

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
When the requested port is `0`, the server SHALL listen on a port assigned by the operating system and SHALL use the actually bound port everywhere it reports or derives its address, including its startup output (see `cli`), `daemon.json`, and its own origin (see `access-control`). `splitty daemon start` MUST bind its listener before it detaches, so that a bind failure is reported by the invoking command (see `cli`) and leaves no background process.

#### Scenario: OS-assigned port reported
- **WHEN** `splitty daemon start --addr 127.0.0.1:0` is run
- **THEN** the printed `SPLITTY_PORT=<port>` line carries the non-zero port the daemon listens on
- **AND** the `port` field of `daemon.json` holds the same port

#### Scenario: Port already in use
- **WHEN** `splitty daemon start` is run while another process listens on the requested port
- **THEN** the command fails in the foreground
- **AND** no background process remains

### Requirement: Desktop Stdio Lifetime Binding
When started with `--desktop-stdio`, an embedded server SHALL treat its stdin as the launcher's control channel: it SHALL shut down when stdin reaches end-of-file, and for every stdin line that reads `bootstrap` it SHALL print a fresh `SPLITTY_BOOTSTRAP=<code>` line. Other input SHALL be ignored, and unterminated input beyond 4096 bytes SHALL be discarded. Without `--desktop-stdio`, stdin SHALL NOT affect the server's lifetime.

#### Scenario: Launcher exits
- **WHEN** the process holding the write end of the server's stdin closes it
- **THEN** the server shuts down and closes all of its sessions

#### Scenario: Launcher requests a fresh code
- **WHEN** the launcher writes the line `bootstrap` to the server's stdin
- **THEN** the server prints exactly one new `SPLITTY_BOOTSTRAP=<code>` line on stdout
- **AND** keeps running

### Requirement: Embedded Mode Lifetime
An embedded server SHALL keep its workspace and sessions only in memory, for the lifetime of its process. It SHALL NOT read a persisted workspace at startup, SHALL NOT write one, and SHALL NOT register itself in `daemon.json`. When `splitty serve` receives an interrupt (Ctrl+C), or reaches end-of-file on stdin with `--desktop-stdio`, it SHALL close all sessions and stop.

#### Scenario: Serve starts empty
- **WHEN** `splitty serve` starts while a `state.json` from a daemon exists in the configuration directory
- **THEN** the embedded server starts with an empty workspace and leaves `state.json` untouched

#### Scenario: Interrupt ends sessions
- **WHEN** `splitty serve` receives Ctrl+C
- **THEN** every terminal session it hosts is closed before the process exits

### Requirement: Desktop App Hosting On macOS And Linux
On macOS and Linux, the desktop app SHALL use a running daemon when `daemon.json` names a live process and that daemon issues a bootstrap code to the app. Without a live daemon, when sessions of its configuration directory are kept (see `session-keepers`), it SHALL first start the daemon with the executable those keepers run, or `splitty` from its `PATH` when that is gone, and use it once it serves within 15 seconds. Otherwise it SHALL start an embedded server inside the app process on an OS-assigned loopback port, and that server SHALL live exactly as long as the app. A `daemon.json` whose process is not alive SHALL be removed before falling back.

#### Scenario: Daemon available
- **WHEN** the desktop app starts while a daemon is running
- **THEN** its window connects to the daemon's port instead of starting an embedded server
- **AND** closing the app leaves the daemon and its sessions running

#### Scenario: No daemon
- **WHEN** the desktop app starts and no `daemon.json` exists
- **THEN** it starts an embedded server on `127.0.0.1` with an OS-assigned port and connects to it

#### Scenario: Daemon does not issue a code
- **WHEN** `daemon.json` names a live process but the request for a bootstrap code fails
- **THEN** the app falls back to an embedded server

#### Scenario: Stale daemon file
- **WHEN** the desktop app finds a `daemon.json` whose process is not alive
- **THEN** it removes the file and starts an embedded server

#### Scenario: Kept sessions after a daemon crash
- **WHEN** the daemon crashed while its panes ran shells, and the desktop app starts
- **THEN** the app starts the daemon, which adopts the kept sessions, and its window shows them

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
The native Windows app SHALL run its backend inside the selected WSL 2 distribution as an embedded server started with `serve --addr 127.0.0.1:0 --desktop-stdio`, and SHALL bind the backend's lifetime to the app through the backend's stdin. It SHALL NOT look for a daemon. On exit, the app SHALL close the backend's stdin, wait up to 3 seconds for it to stop, then terminate only its own bridge process; it MUST NOT terminate the WSL distribution. While a backend for the selected distribution is alive, a further start request SHALL reuse it. Starting a backend in a different distribution SHALL stop the previous one. The app SHALL NOT need WSL for remote windows (see `desktop-remote`): without WSL, or before its backend runs, its setup page SHALL still offer to connect to a host.

#### Scenario: App exit stops the backend
- **WHEN** the user quits the Windows app
- **THEN** the backend in WSL receives end-of-file on stdin and shuts down, closing its sessions

#### Scenario: Backend does not stop in time
- **WHEN** the backend has not exited 3 seconds after its stdin was closed
- **THEN** the app terminates the bridge process
- **AND** leaves the WSL distribution and unrelated processes in it running

#### Scenario: Repeated start for the same distribution
- **WHEN** the app requests a backend start for the distribution whose backend is still running
- **THEN** no second backend is started

#### Scenario: Remote only
- **WHEN** the Windows app runs on a machine without WSL and the user connects to a host
- **THEN** a remote window shows the host, and no WSL backend is started

### Requirement: Single Daemon Instance
At most one daemon SHALL be registered per user configuration directory. If `daemon.json` names a live process, `splitty daemon start` SHALL report the running daemon (see `cli`) and exit successfully without starting another daemon. If the named process is not alive, the stale file SHALL be removed and the start SHALL proceed. Embedded servers are not subject to this limit. Daemon mode SHALL be refused with a non-zero exit status on platforms other than Unix.

#### Scenario: Daemon already running
- **WHEN** `splitty daemon start` is run while the daemon recorded in `daemon.json` is alive
- **THEN** it reports the running daemon's pid and port and exits with status 0
- **AND** no second daemon process exists

#### Scenario: Stale registration
- **WHEN** `splitty daemon start` finds a `daemon.json` whose pid is not alive
- **THEN** it removes the file and starts a new daemon

### Requirement: Daemon Detachment
`splitty daemon start` SHALL detach the daemon from the invoking terminal and session so that it keeps running after the terminal closes. The invoking command SHALL return only once the daemon is serving: its HTTP listener accepts connections, its control socket is bound or was deliberately skipped, and `daemon.json` is written. If the daemon fails before that point, the invoking command SHALL report the failure (see `cli`) and SHALL leave no `daemon.json` that names the failed process. The daemon SHALL append its log output to `daemon.log` in the Splitty configuration directory.

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
A running daemon SHALL advertise itself in `daemon.json` in the Splitty configuration directory (`$XDG_CONFIG_HOME/splitty`, by default `~/.config/splitty` on Linux). The file SHALL contain the daemon's `pid`, its listening `port`, the path of its local control `socket`, `startedAt`, the server `version`, its `protocol` version (see `remote-hosts`) and `keepsSessions: true`, which tells that its sessions outlive it (see `session-keepers`); a file without `protocol` counts as protocol 1, and one without `keepsSessions` as a daemon whose sessions end with it. The daemon process itself SHALL write the file after detaching, once it is ready to serve. The `socket` field SHALL name a control socket that exists, or be empty when the daemon runs without one. The file SHALL NOT contain any credential; its permissions and the control socket are governed by `access-control`, and how clients use it by `cli`. A missing or unparsable file SHALL be treated as "no daemon", and a file whose `pid` is not a live process SHALL be treated as stale.

#### Scenario: Running daemon advertised
- **WHEN** a daemon has started
- **THEN** `daemon.json` holds that daemon's pid, its bound port, its control socket path, its start time, its version, its protocol version and `keepsSessions: true`
- **AND** holds no token

#### Scenario: Corrupt info file
- **WHEN** `daemon.json` contains invalid JSON
- **THEN** `splitty daemon status` reports `No daemon running`

#### Scenario: Control socket unavailable
- **WHEN** the daemon cannot create its control socket
- **THEN** it still starts and serves its HTTP listener
- **AND** the `socket` field of `daemon.json` is empty

### Requirement: Daemon Graceful Shutdown
On SIGTERM or SIGINT, the daemon SHALL stop accepting connections, release the keepers of its sessions without ending the sessions (see `session-keepers`), write the final workspace state to `state.json`, remove `daemon.json`, and remove its control socket before exiting.

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
