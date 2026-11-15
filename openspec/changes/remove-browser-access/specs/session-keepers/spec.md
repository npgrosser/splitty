## MODIFIED Requirements

### Requirement: Keeper Per Daemon Session
In daemon mode, every terminal session SHALL run under a keeper process of its own, started by the daemon as `splitty session-keeper`, which owns the session's terminal and program and outlives the daemon process. When a keeper cannot be started, the daemon SHALL run that session in its own process and log why. An embedded server SHALL keep every session's terminal in its own process.

#### Scenario: Daemon killed
- **WHEN** the daemon is killed with SIGKILL while a pane runs a shell
- **THEN** the shell keeps running with the same PID

#### Scenario: Embedded server
- **WHEN** the desktop app's embedded service hosts a session
- **THEN** no keeper process exists for it, and the session ends with the server

#### Scenario: Keeper cannot start
- **WHEN** the daemon cannot use its keeper directory
- **THEN** new sessions still start, in the daemon's own process, and `daemon.log` names the reason
