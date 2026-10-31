# session-keepers Specification

## Purpose
Keeps a daemon's terminal sessions alive independently of the daemon process: each session's program runs under a keeper process that survives daemon restarts, updates and crashes, and a starting daemon adopts the live keepers instead of starting the programs again.

## Requirements

### Requirement: Keeper Per Daemon Session
In daemon mode, every terminal session SHALL run under a keeper process of its own, started by the daemon as `splitty session-keeper`, which owns the session's terminal and program and outlives the daemon process. When a keeper cannot be started, the daemon SHALL run that session in its own process and log why. An embedded server SHALL keep every session's terminal in its own process.

#### Scenario: Daemon killed
- **WHEN** the daemon is killed with SIGKILL while a pane runs a shell
- **THEN** the shell keeps running with the same PID

#### Scenario: Embedded server
- **WHEN** `splitty serve` hosts a session
- **THEN** no keeper process exists for it, and the session ends with the server

#### Scenario: Keeper cannot start
- **WHEN** the daemon cannot use its keeper directory
- **THEN** new sessions still start, in the daemon's own process, and `daemon.log` names the reason

### Requirement: Adoption On Daemon Start
When a daemon starts, it SHALL adopt every live keeper of its run directory that speaks its keeper protocol, under the keeper's session id, and SHALL fill the session's scrollback from the keeper's buffer before clients can attach. A persisted session with a live keeper SHALL NOT be started again. An adopted session SHALL keep its process, PID, terminal and PTY size.

#### Scenario: Restart keeps the process and its output
- **WHEN** a daemon whose pane printed `before` and then started `sleep 1000` is stopped and started again
- **THEN** the pane's session still runs the same `sleep` process
- **AND** a client attaching to it receives `before` in the replay

#### Scenario: Keeper gone
- **WHEN** a persisted session's keeper is no longer alive when the daemon starts
- **THEN** the session is started again as `server-modes` specifies (Session Respawn On Daemon Start)

### Requirement: Exit Results Survive The Daemon
A keeper whose program exits SHALL keep the exit result and the final output until a controller has acknowledged them, at most 7 days. A daemon adopting such a keeper SHALL report the session as exited with that result.

#### Scenario: Exit while no daemon runs
- **WHEN** a pane's program exits with code 3 while no daemon runs, and a daemon starts afterwards
- **THEN** a client attaching to that session receives the program's final output and a `closed` event with `exitCode` 3

### Requirement: One Controller At A Time
A keeper SHALL serve one controller at a time. A further claim SHALL be refused while a controller holds the keeper, unless it takes the keeper over; the previous controller SHALL then be told and disconnected. A daemon SHALL take over every keeper of its run directory, also one it loses while it runs. A keeper without a controller SHALL keep reading its program's output.

#### Scenario: A second terminal is refused
- **WHEN** no daemon runs, one terminal is attached directly to a keeper, and a second `splitty attach` targets the same session
- **THEN** the second command fails saying that another client controls the session, and the first one keeps working

#### Scenario: Busy program without a controller
- **WHEN** a keeper's program writes more output than the PTY buffer holds while no controller holds the keeper
- **THEN** the program keeps running and the newest output is in the keeper's buffer

### Requirement: Output Exactly Once Across Controllers
A controller resuming at an offset SHALL receive the keeper's output from that offset exactly once, as far as the keeper's buffer holds it; output older than the buffer SHALL be reported as lost, not replayed in part. A daemon that reconnects to a keeper while it runs SHALL resume where it stopped, so its clients see no byte twice and miss none the buffer held.

#### Scenario: Reconnect after a slow controller
- **WHEN** a keeper disconnects a controller whose unsent output exceeded its bound while the program keeps printing numbered lines, and the controller resumes at its last offset
- **THEN** the controller receives every line still in the buffer once, in order, and is told whether lines were lost

### Requirement: Keeper Protocol Compatibility
A daemon SHALL adopt the keepers of any Splitty version that speaks keeper protocol version 1. A keeper announcing another protocol version SHALL be left running and not adopted, and the daemon SHALL log its session id.

#### Scenario: Unknown protocol version
- **WHEN** a daemon starts next to a keeper whose `hello` announces protocol version 2
- **THEN** the keeper keeps running, the daemon does not adopt it and logs its session id

### Requirement: Sessions Without A Pane
A daemon SHALL adopt a keeper whose session the persisted workspace does not know as a session without a pane: clients can attach to it by its session id and `splitty attach` lists it, but it SHALL NOT be placed into the layout.

#### Scenario: Lost workspace file
- **WHEN** a daemon starts without `state.json` while a keeper of an earlier daemon runs
- **THEN** the workspace has no pane for it, and `splitty attach <session-id>` shows the keeper's session
