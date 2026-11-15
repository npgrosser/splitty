## MODIFIED Requirements

### Requirement: Shell Program Selection
When the server spawns a session for a pane, it SHALL choose the program to run in this order: (1) the pane session's `shellProfileId`, if it names a known profile that is enabled and available; (2) the legacy `shell` path, when no `shellProfileId` is given; (3) the default profile, which is the configured default if enabled and available, otherwise the system default profile if enabled and available, otherwise the first enabled and available profile. A requested profile that is missing, disabled or unavailable SHALL fall back to the default profile. The `mux.pane_spawned` event SHALL report the effective `shellProfileId` and, when fallback occurred, a `shellProfileFallbackReason`. Profile arguments MUST be passed to the program as literal, individual arguments in their configured order without shell interpolation.

#### Scenario: Profile id takes precedence over legacy shell
- **WHEN** a pane's session names both a valid `shellProfileId` and a `shell` path
- **THEN** the session runs the profile's command with the profile's arguments

#### Scenario: Unknown profile id falls back to the default profile
- **WHEN** a pane's session names a `shellProfileId` that matches no configured profile
- **THEN** the session runs the default profile
- **AND** `mux.pane_spawned` reports the default profile's id as `shellProfileId` and `shellProfileFallbackReason` as `missing`

#### Scenario: Unavailable known profile falls back to the default profile
- **WHEN** a pane's session names a known disabled or unavailable `shellProfileId`
- **THEN** the session runs the default profile
- **AND** `mux.pane_spawned` reports the effective profile id and `shellProfileFallbackReason` as `disabled` or `unavailable`

#### Scenario: Legacy shell gets login arguments
- **WHEN** a pane's session names `shell` `/bin/bash` and no `shellProfileId`
- **THEN** the session runs `/bin/bash` with the login argument `-l`

#### Scenario: No usable profile
- **WHEN** a pane's session names no `shellProfileId` or `shell` and no profile is enabled and available
- **THEN** no session starts and the originating client receives `mux.op_failed` for the pane (see `mux-protocol`, Pane Spawn Notification)

### Requirement: Startup Command and Restore Mode
When a pane's session names a `command` with a character other than whitespace, the server SHALL run it according to the session's `restoreMode`: `replay` (also the default for an empty or unrecognized value) starts the shell normally and types the command followed by a newline into the terminal shortly after start; `shell` runs the command through the shell and then continues with an interactive shell; `close` runs the command through the shell and lets the session end when the command ends. The `shell` and `close` modes MUST only be used for `sh`, `bash`, `zsh`, `dash` and `ksh`; for any other shell the server SHALL fall back to `replay`. A `command` that is empty or holds only whitespace SHALL be no command, whatever `restoreMode` says.

#### Scenario: Replay mode types the command
- **WHEN** a pane's session is spawned with `command` `make build` and `restoreMode` `replay`
- **THEN** the shell starts interactively and `make build` followed by a newline is written to the terminal after the shell has had time to start

#### Scenario: Shell mode keeps an interactive shell
- **WHEN** a pane's bash session is spawned with `command` `htop` and `restoreMode` `shell`
- **THEN** bash runs `htop` and, after it exits, the session continues in an interactive bash

#### Scenario: Close mode ends with the command
- **WHEN** a pane's zsh session is spawned with `command` `make build` and `restoreMode` `close`
- **THEN** the session's process ends when `make build` ends

#### Scenario: Unsupported shell falls back to replay
- **WHEN** a pane's fish session is spawned with a `command` and `restoreMode` `shell`
- **THEN** the command is typed into the running shell as in `replay` mode

#### Scenario: Blank command
- **WHEN** a pane's session is spawned with `command` consisting of two spaces and `restoreMode` `close`
- **THEN** the shell starts interactively as without a command, and nothing is typed into the terminal

### Requirement: Working Directory
The server SHALL start a pane's session in the directory its session names (`cwd`), resolved as follows: a relative directory of a template's pane is joined onto the root the template is applied with (see `templates`); a leading `~` expands to the server user's home directory; an empty, missing or non-directory path falls back to the home directory.

#### Scenario: Relative directory with root
- **WHEN** a template whose pane names the directory `src` is applied with the root `/home/me/project`
- **THEN** the pane's session starts in `/home/me/project/src` if that directory exists

#### Scenario: Tilde expansion
- **WHEN** a pane's session is spawned with `cwd` `~/code`
- **THEN** the session starts in the `code` directory under the server user's home

#### Scenario: Nonexistent directory
- **WHEN** a pane's session is spawned with a `cwd` that does not exist
- **THEN** the session starts in the server user's home directory instead of failing

### Requirement: Session Environment
The server SHALL start every session with `TERM=xterm-256color`, `COLORTERM=truecolor`, `FORCE_HYPERLINK=1` and `SPLITTY_SESSION_ID` set to the session id. `FORCE_HYPERLINK` makes programs that detect hyperlink support from the environment send OSC 8 hyperlinks, which open the whole target even where the program wraps a link across lines itself. It SHALL set `SPLITTY_PANE_ID` and `SPLITTY_SCREEN_ID` to the pane and screen the session was spawned for. Ordinary sessions SHALL receive `SPLITTY_SOCK` when configured; agent-owned managed jobs SHALL instead receive only their `SPLITTY_AGENT_SOCK` as specified in `access-control`. Which values MUST NOT appear in a session's environment is defined in `access-control` (Pane Environment Carries No Credential).

#### Scenario: Pane-bound session environment
- **WHEN** the server spawns an ordinary session for pane `p1` of screen `s1`
- **THEN** the session's process sees `SPLITTY_PANE_ID=p1`, `SPLITTY_SCREEN_ID=s1`, `SPLITTY_SESSION_ID` and `SPLITTY_SOCK`

### Requirement: Terminal Output Streaming
The server SHALL deliver session output only to connections currently streaming that session, meaning connections that attached it with `attach` and have not since detached or disconnected. Output MUST be sent as binary output frames as defined in `mux-protocol` (Binary Terminal Output Frames), carrying the raw bytes exactly as read from the terminal, except status probes, which the server answers itself and removes before it buffers or sends the output (see `program-status`, Probe Removed from the Output).

#### Scenario: Output delivered as a binary frame
- **WHEN** the process in session `abc` writes `hello` to its terminal while a client streams `abc`
- **THEN** that client receives a binary output frame for session `abc` whose payload is `hello`

#### Scenario: Non-streaming connections receive nothing
- **WHEN** a session produces output
- **THEN** connections that have not attached that session receive no output frames for it

#### Scenario: Status probe removed
- **WHEN** the process in a streamed session writes `a`, `ESC ] 7501 ; ? BEL` and `b`
- **THEN** the client receives `ab` and no part of the probe

### Requirement: Attaching to an Existing Session
On `attach` with the `id` of an existing session, the server SHALL send that session's buffered output as binary output frames of at most 32 KiB each, preceded in those frames by the sequences that restore the terminal modes the buffered output starts in (see Terminal Modes in the Replay), then send an `attached` event with the `sessionId`, and then stream new output to the connection. Buffered output MUST be sent before any new output, and the buffered output and the live stream SHALL together contain every byte the session produced exactly once. If the same connection already streams the session, the previous stream SHALL be replaced so that output is neither duplicated nor split between streams. Several connections MAY stream the same session at the same time, each receiving the complete output. Detaching, disconnecting or replacing a stream MUST NOT end or interrupt the session's output for later or other streams. `attach` MUST NOT change the PTY size; `cols` and `rows` in `attach` are ignored, and clients change the size with `resize`.

#### Scenario: Reattach replays buffered output
- **WHEN** a client attaches to a session that has 70 KiB of buffered output
- **THEN** it receives that output as three binary frames of at most 32 KiB each, in order
- **AND** then receives an `attached` event for the session

#### Scenario: Attach to an unknown session
- **WHEN** a client sends `attach` with an `id` that names no session
- **THEN** the server sends an `error` event with that `sessionId` and the message `session not found`

#### Scenario: Attach does not resize
- **WHEN** a client sends `attach` with `cols` 200 and `rows` 50
- **THEN** the session's PTY size is unchanged

#### Scenario: Reconnect keeps the session streaming
- **WHEN** a client streams a running session, disconnects, reconnects and attaches to the session again
- **THEN** it receives the replay followed by new output
- **AND** it receives no `closed` event while the process is running

#### Scenario: Two connections stream one session
- **WHEN** two connections attach to the same session and the process prints a line
- **THEN** both connections receive that line

#### Scenario: No gap between replay and live output
- **WHEN** a session produces output continuously while a client attaches
- **THEN** the bytes the client receives after the restored modes, buffered output followed by live output, contain no byte twice and miss none produced after the oldest replayed byte

#### Scenario: Replay after the buffer wrapped
- **WHEN** a program enabled bracketed paste and then printed more than the scrollback buffer holds, and a client attaches to its session
- **THEN** the replay begins with `CSI ? 2004 h`, ahead of the buffered output

### Requirement: Session Lifetime Independent of Connections
A client disconnecting SHALL stop all output streams of that connection and MUST NOT terminate any session. A session ends only when its pane is closed through a workspace operation (see `workspace-model`), when an embedded server shuts down or a daemon's sessions are ended (see `server-modes`), or when its process exits.

#### Scenario: Disconnect leaves sessions running
- **WHEN** a client with two streamed sessions disconnects
- **THEN** both sessions keep running on the server
- **AND** a later client can attach to them by id

#### Scenario: Daemon stop leaves sessions running
- **WHEN** a daemon is stopped with `splitty daemon stop` while a client streams one of its sessions
- **THEN** the session's process keeps running, and a client of the next daemon can attach to the session by the same id

### Requirement: Terminal Resize
On `resize` with a session `id`, `cols` and `rows`, the server SHALL record that size as the size at which the requesting connection shows the session, and SHALL set the session's PTY to the size its PTY size policy derives from the recorded sizes (see PTY Size Policy). The server SHALL answer every `resize` with a `size` event for the session to the requesting connection, carrying the session's PTY size after the report, also when the size did not change. When a `resize` carries `redraw: true` and the PTY size does not change, the server SHALL resize the PTY by one row and back to its size, so the session's processes redraw, without sending a `size` event for the intermediate size. A `resize` with `cols` or `rows` of 0 SHALL change nothing and be answered with an `error` event for the session.

#### Scenario: Resize applies to the PTY
- **WHEN** the only client showing a session sends `{"type":"resize","id":"<sid>","cols":100,"rows":30}`
- **THEN** the session's terminal reports 100 columns and 30 rows to its processes
- **AND** the client receives a `size` event with `sessionId` `<sid>`, `cols` 100 and `rows` 30

#### Scenario: Resize of an unknown session
- **WHEN** a client sends `resize` with an `id` that names no session
- **THEN** the server sends an `error` event with that `sessionId` and the message `session not found`

#### Scenario: Unchanged size is answered
- **WHEN** a client's `resize` does not change the session's PTY size because another active client shows it smaller
- **THEN** the PTY size stays the same
- **AND** the client receives a `size` event with the unchanged size

#### Scenario: Redraw without a size change
- **WHEN** a client sends `resize` with `redraw: true` and the PTY size stays 100 columns and 30 rows
- **THEN** the PTY is resized to 100 columns and 31 rows and back to 100 columns and 30 rows
- **AND** no connection receives a `size` event with 31 rows

### Requirement: Process Exit
When a session's process exits, the server SHALL send each connection streaming that session a `closed` event with the `sessionId` and the actual `exitCode` when available, and SHALL stop streaming the session after draining its available final output. A nonzero exit code MUST NOT be collapsed to a generic failure value. Signal termination SHALL carry `signal` when available; an unavailable code SHALL be absent rather than falsely zero. The exited session SHALL remain registered until its pane is closed, including automatic closure according to the owning host policy. The `session.exited` event is a notification and is outside this capability. An ordinary mux pane whose exit matches the owning server close_on_exit setting SHALL be closed once its process, seen running by the server, has exited and its output has ended, without waiting for a periodic sample; while its terminal stays open, or for an exit that happened while no server observed it, it SHALL be closed after a bounded grace for final output. This holds also when no clients are attached.

#### Scenario: Shell exits
- **WHEN** the user types `exit` in a streamed session's shell
- **THEN** the streaming client receives `closed` with `exitCode` 0

#### Scenario: Command fails
- **WHEN** a pane's session spawned with `restoreMode` `close` runs a command that exits with a failure status
- **THEN** the streaming client receives `closed` with that actual non-zero `exitCode`

#### Scenario: Exited session can still be closed
- **WHEN** a client sends `close_pane` for a pane whose session's process has already exited
- **THEN** the server removes the pane and its session, and a later `attach` to that session id fails with `session not found`

#### Scenario: Exact managed-process result
- **WHEN** a directly executed job program exits with code 23
- **THEN** its job state and streamed `closed` event both report 23

#### Scenario: Pane closes with its exit
- **WHEN** an ordinary pane's shell exits under close_on_exit `always` and its output ends
- **THEN** the pane is closed at once, with the shell's last output recorded before

#### Scenario: Terminal held open after the exit
- **WHEN** a pane's shell exits while a process it left behind keeps the terminal open
- **THEN** the pane is closed after the final-output grace

### Requirement: Image Paste In A Pane
When the user pastes into a pane and the clipboard data holds an image file and no text, the client SHALL send each image with `save_image` to the server of that pane's connection and SHALL paste the returned path into that pane as a bracketed paste, several paths separated by spaces. Ctrl+V without other modifiers SHALL be pasted this way except on macOS; when the clipboard then holds neither an image nor another file, the client SHALL send `^V` to the terminal as before. Other files and all dropped images SHALL follow Files Into A Pane. A paste or drop without a file SHALL behave as before. When the server answers `image_failed`, or does not know `save_image`, the client SHALL say so in a notice and paste nothing; for a server that does not know it, the notice SHALL say that it needs an update.

#### Scenario: Screenshot into Claude Code on a host
- **WHEN** the clipboard holds a screenshot and the user presses Ctrl+V in a pane of a remote screen of `vm1`
- **THEN** the image is stored on `vm1`, its path is pasted into that pane as a bracketed paste, and nothing is sent to any other server

#### Scenario: Ctrl+V with text
- **WHEN** the clipboard holds text and the user presses Ctrl+V in a pane
- **THEN** the terminal receives `^V` and no `save_image` is sent

#### Scenario: Host with an older Splitty
- **WHEN** the user pastes an image into a pane of a host whose server answers `save_image` with "unknown message type"
- **THEN** a notice says the host needs an update, and nothing is pasted

### Requirement: Default Shell Profile
The system default profile SHALL be the detected profile for the server's login shell (`$SHELL`), or the first detected profile when `$SHELL` names no executable. An empty `[shells] default_profile_id` in `config.toml` SHALL mean that the default follows the system default, resolved again whenever the profiles are reconciled; a non-empty value is an explicit choice, replaced by the first enabled and available profile when it names none. Reconciling profiles with detection MUST NOT write a resolved id into an empty value. `GET /api/shell-profiles` SHALL report the effective `defaultProfileId`, `followSystemDefault` (true exactly when the stored value is empty) and `systemDefaultProfileId`.

#### Scenario: New configuration follows the login shell
- **WHEN** the server starts without a stored default and `$SHELL` is `/usr/bin/zsh`
- **THEN** `GET /api/shell-profiles` reports the zsh profile as `defaultProfileId` and `systemDefaultProfileId` and `followSystemDefault` true
- **AND** `config.toml` keeps `default_profile_id` empty

#### Scenario: Login shell changes
- **WHEN** the default follows the system default and the server starts again with `$SHELL` set to `/bin/bash`
- **THEN** the default profile is the bash profile

#### Scenario: Explicit choice survives a login shell change
- **WHEN** `config.toml` stores `default_profile_id` naming the bash profile
- **AND** the server starts again with `$SHELL` set to `/usr/bin/zsh`
- **THEN** the default profile is still the bash profile and `followSystemDefault` is false

#### Scenario: Switching back to the system default
- **WHEN** `config.toml` stores an empty `default_profile_id` and the server starts
- **THEN** `GET /api/shell-profiles` reports the system default profile as `defaultProfileId` and `followSystemDefault` true

## REMOVED Requirements

### Requirement: Session Creation
**Reason**: The session message `create` and its `created` event were used only by the web client; sessions start for panes through workspace operations and `mux.pane_spawned` (see `mux-protocol`, Pane Spawn Notification).
**Migration**: Create a pane with a workspace operation and attach to the session `mux.pane_spawned` names.

### Requirement: Session Creation Failure
**Reason**: Goes with `create`; a failed pane spawn is reported with `mux.op_failed` (see `mux-protocol`, Pane Spawn Notification).
**Migration**: None.

### Requirement: Closing a Session
**Reason**: The session message `close` was used only by the web client; a session ends with its pane (`close_pane`, closing a screen) or its process.
**Migration**: Close the pane with `close_pane`.

### Requirement: Session Metadata Updates
**Reason**: The `title` event was read only by the web client; windows take `displayTitle` and `busy` from session snapshots (see `mux-protocol`, Pane State From Session Snapshots).
**Migration**: Read the session's `displayTitle` and `busy` from `mux.screen_snapshot`.

### Requirement: Managed Pane Creation Cannot Fall Back
**Reason**: Its subject, a session `create` for a job pane, no longer exists; a job's pane gets its session only from the job launcher (see `managed-jobs`).
**Migration**: None.
