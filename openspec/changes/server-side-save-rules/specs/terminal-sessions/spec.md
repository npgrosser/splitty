## MODIFIED Requirements

### Requirement: Startup Command and Restore Mode
When `create` includes a `command` with a character other than whitespace, the server SHALL run it according to `restoreMode`: `replay` (also the default for an empty or unrecognized value) starts the shell normally and types the command followed by a newline into the terminal shortly after start; `shell` runs the command through the shell and then continues with an interactive shell; `close` runs the command through the shell and lets the session end when the command ends. The `shell` and `close` modes MUST only be used for `sh`, `bash`, `zsh`, `dash` and `ksh`; for any other shell the server SHALL fall back to `replay`. A `command` that is empty or holds only whitespace SHALL be no command, whatever `restoreMode` says.

#### Scenario: Replay mode types the command
- **WHEN** a client creates a session with `command` `make build` and `restoreMode` `replay`
- **THEN** the shell starts interactively and `make build` followed by a newline is written to the terminal after the shell has had time to start

#### Scenario: Shell mode keeps an interactive shell
- **WHEN** a client creates a bash session with `command` `htop` and `restoreMode` `shell`
- **THEN** bash runs `htop` and, after it exits, the session continues in an interactive bash

#### Scenario: Close mode ends with the command
- **WHEN** a client creates a zsh session with `command` `make build` and `restoreMode` `close`
- **THEN** the session's process ends when `make build` ends

#### Scenario: Unsupported shell falls back to replay
- **WHEN** a client creates a fish session with a `command` and `restoreMode` `shell`
- **THEN** the command is typed into the running shell as in `replay` mode

#### Scenario: Blank command
- **WHEN** a client creates a session with `command` consisting of two spaces and `restoreMode` `close`
- **THEN** the shell starts interactively as without a command, and nothing is typed into the terminal
