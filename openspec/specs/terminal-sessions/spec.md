# terminal-sessions Specification

## Purpose
Terminal sessions are server-side pseudo-terminals running a shell or command on behalf of panes. This capability defines how sessions are started (program, arguments, working directory, environment), how clients exchange input, output, size and lifecycle messages for them, and how long a session lives relative to client connections. Shell integration, OSC 133 prompt tracking, bells and notifications are out of scope.

## Requirements

### Requirement: Session Creation
The server SHALL start a new terminal session when a client sends `create`, assign it a server-generated session id, and answer with a `created` event carrying that `sessionId`, the resolved `shellProfileId` (absent when a legacy `shell` was used), and the server user's `homeDir`. The PTY SHALL start with the requested `cols`/`rows`, defaulting to 80x24 when either is missing or zero. Payload shapes are defined in `docs/protocol.ts`.

#### Scenario: Successful create
- **WHEN** a client sends `{"type":"create","cols":120,"rows":40}`
- **THEN** the server starts a PTY of size 120x40 running the selected shell
- **AND** sends a `created` event whose `sessionId` identifies the new session

#### Scenario: Missing size falls back to defaults
- **WHEN** a client sends `create` without `cols` and `rows`
- **THEN** the session's PTY is 80 columns by 24 rows

#### Scenario: Created session streams to the creating connection
- **WHEN** a session is created by a client's `create`
- **THEN** that connection receives the session's output without sending `attach`

### Requirement: Session Creation Failure
If a session cannot be started, the server SHALL send an `error` event without a `sessionId` and SHALL NOT send `created`.

#### Scenario: Requested profile is disabled
- **WHEN** a client sends `create` with a `shellProfileId` naming a profile that exists but is disabled or unavailable
- **THEN** the server sends an `error` event with no `sessionId`
- **AND** no session is started

#### Scenario: Program cannot be spawned
- **WHEN** the resolved shell program cannot be executed
- **THEN** the server sends an `error` event with no `sessionId` describing the spawn failure

### Requirement: Shell Program Selection
The server SHALL choose the program to run in this order: (1) the requested `shellProfileId`, if it names a known profile that is enabled and available; (2) the legacy `shell` path, when no `shellProfileId` is given; (3) the default profile, which is the configured default if enabled and available, otherwise the system default profile if enabled and available, otherwise the first enabled and available profile. A requested profile that is missing, disabled or unavailable SHALL fall back to the default profile. The `created` or `mux.pane_spawned` event SHALL report the effective `shellProfileId` and, when fallback occurred, a `shellProfileFallbackReason`. Profile arguments MUST be passed to the program as literal, individual arguments in their configured order without shell interpolation.

#### Scenario: Profile id takes precedence over legacy shell
- **WHEN** a client sends `create` with both a valid `shellProfileId` and a `shell` path
- **THEN** the session runs the profile's command with the profile's arguments

#### Scenario: Unknown profile id falls back to the default profile
- **WHEN** a client sends `create` with a `shellProfileId` that matches no configured profile
- **THEN** the session runs the default profile
- **AND** `created` or `mux.pane_spawned` reports the default profile's id as `shellProfileId` and `shellProfileFallbackReason` as `missing`

#### Scenario: Unavailable known profile falls back to the default profile
- **WHEN** a client sends `create` with a known disabled or unavailable `shellProfileId`
- **THEN** the session runs the default profile
- **AND** `created` or `mux.pane_spawned` reports the effective profile id and `shellProfileFallbackReason` as `disabled` or `unavailable`

#### Scenario: Legacy shell gets login arguments
- **WHEN** a client sends `create` with `shell` set to `/bin/bash` and no `shellProfileId`
- **THEN** the session runs `/bin/bash` with the login argument `-l`
- **AND** `created` carries no `shellProfileId`

#### Scenario: No usable profile
- **WHEN** no `shellProfileId` or `shell` is given and no profile is enabled and available
- **THEN** session creation fails as described in Session Creation Failure

### Requirement: Startup Command and Restore Mode
When `create` includes a non-empty `command`, the server SHALL run it according to `restoreMode`: `replay` (also the default for an empty or unrecognized value) starts the shell normally and types the command followed by a newline into the terminal shortly after start; `shell` runs the command through the shell and then continues with an interactive shell; `close` runs the command through the shell and lets the session end when the command ends. The `shell` and `close` modes MUST only be used for `sh`, `bash`, `zsh`, `dash` and `ksh`; for any other shell the server SHALL fall back to `replay`.

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

### Requirement: Working Directory
The server SHALL start the session in the requested working directory, resolved as follows: a relative `dir` is joined onto `root` when `root` is given; a leading `~` expands to the server user's home directory; an empty, missing or non-directory path falls back to the home directory.

#### Scenario: Relative directory with root
- **WHEN** a client sends `create` with `dir` `src` and `root` `/home/u/project`
- **THEN** the session starts in `/home/u/project/src` if that directory exists

#### Scenario: Tilde expansion
- **WHEN** a client sends `create` with `dir` `~/code`
- **THEN** the session starts in the `code` directory under the server user's home

#### Scenario: Nonexistent directory
- **WHEN** a client sends `create` with a `dir` that does not exist
- **THEN** the session starts in the server user's home directory instead of failing

### Requirement: Session Environment
The server SHALL start every session with `TERM=xterm-256color`, `COLORTERM=truecolor`, `FORCE_HYPERLINK=1` and `SPLITTY_SESSION_ID` set to the session id. `FORCE_HYPERLINK` makes programs that detect hyperlink support from the environment send OSC 8 hyperlinks, which open the whole target even where the program wraps a link across lines itself. It SHALL set `SPLITTY_PANE_ID` and `SPLITTY_SCREEN_ID` when the session was spawned for a pane or the `create` request names a `paneId` or `screenId`. Ordinary sessions SHALL receive `SPLITTY_SOCK` when configured; agent-owned managed jobs SHALL instead receive only their `SPLITTY_AGENT_SOCK` as specified in `access-control`. Which values MUST NOT appear in a session's environment is defined in `access-control` (Pane Environment Carries No Credential).

#### Scenario: Pane-bound session environment
- **WHEN** a client sends ordinary session `create` with `paneId` `p1` and `screenId` `s1`
- **THEN** the session's process sees `SPLITTY_PANE_ID=p1`, `SPLITTY_SCREEN_ID=s1`, `SPLITTY_SESSION_ID` and `SPLITTY_SOCK`

### Requirement: Sessions Spawned for Panes
When a workspace operation creates a pane that needs a terminal (see `workspace-model`), the server SHALL start the session itself under the session id the workspace assigned to that pane. An ordinary session uses the pane's shell profile, command and working directory; a managed job uses its explicit direct launch. Both start at 80x24 and receive pane/screen identifiers and their applicable control socket environment. Clients learn the pane-to-session mapping from `mux.pane_spawned` only after a successful start (see `mux-protocol`) and connect to the session with `attach`.

#### Scenario: Pane created by a workspace operation
- **WHEN** a workspace operation successfully creates a new terminal pane
- **THEN** a session with the pane's assigned session id is running before any client attaches to it
- **AND** a client that attaches to that id receives its output

### Requirement: Terminal Input
The server SHALL write the UTF-8 bytes of the `data` field of an `input` message unchanged to the PTY of the session named by `id`. Any connection MAY send input to any existing session; it does not need to be streaming that session.

#### Scenario: Input reaches the process
- **WHEN** a client sends `{"type":"input","id":"<sid>","data":"ls\r"}`
- **THEN** the bytes `ls\r` are written to that session's terminal

#### Scenario: Input for an unknown session
- **WHEN** a client sends `input` with an `id` that names no session
- **THEN** the server sends an `error` event with that `sessionId` and the message `session not found`

#### Scenario: Write failure
- **WHEN** writing to the session's terminal fails
- **THEN** the server sends an `error` event with the `sessionId` describing the write failure

### Requirement: Terminal Output Streaming
The server SHALL deliver session output only to connections currently streaming that session, meaning connections that created it with `create` or attached it with `attach` and have not since detached, closed or disconnected. Output MUST be sent as binary output frames as defined in `mux-protocol` (Binary Terminal Output Frames), carrying the raw bytes exactly as read from the terminal.

#### Scenario: Output delivered as a binary frame
- **WHEN** the process in session `abc` writes `hello` to its terminal while a client streams `abc`
- **THEN** that client receives a binary output frame for session `abc` whose payload is `hello`

#### Scenario: Non-streaming connections receive nothing
- **WHEN** a session produces output
- **THEN** connections that have not created or attached that session receive no output frames for it

### Requirement: Scrollback Buffer
The server SHALL keep the most recent output of each session in a fixed-size buffer that discards its oldest bytes when full. The buffer size SHALL be 1 MiB unless the server process environment sets `SPLITTY_SCROLLBACK_SIZE` to a positive integer number of bytes; any other value falls back to the default.

#### Scenario: Buffer keeps only the newest bytes
- **WHEN** a session has produced more output than the buffer size
- **THEN** the buffer holds only the newest bytes, in the order they were produced

#### Scenario: Configured size
- **WHEN** the server is started with `SPLITTY_SCROLLBACK_SIZE=262144`
- **THEN** sessions it starts keep at most 262144 bytes of output

### Requirement: Attaching to an Existing Session
On `attach` with the `id` of an existing session, the server SHALL send that session's buffered output as binary output frames of at most 32 KiB each, preceded in those frames by the sequences that restore the terminal modes the buffered output starts in (see Terminal Modes in the Replay), then send an `attached` event with the `sessionId` and `homeDir`, and then stream new output to the connection. Buffered output MUST be sent before any new output, and the buffered output and the live stream SHALL together contain every byte the session produced exactly once. If the same connection already streams the session, the previous stream SHALL be replaced so that output is neither duplicated nor split between streams. Several connections MAY stream the same session at the same time, each receiving the complete output. Detaching, disconnecting or replacing a stream MUST NOT end or interrupt the session's output for later or other streams. `attach` MUST NOT change the PTY size; `cols` and `rows` in `attach` are ignored, and clients change the size with `resize`.

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

### Requirement: Detaching from a Session
On `detach` with a session `id`, the server SHALL stop streaming that session's output to the connection and SHALL keep the session and its process running. The server sends no response to `detach`, and detaching a session the connection does not stream has no effect.

#### Scenario: Detach keeps the process running
- **WHEN** a client detaches from a session running a long command
- **THEN** the client stops receiving output for that session
- **AND** the command keeps running and the session can be attached again

### Requirement: Session Lifetime Independent of Connections
A client disconnecting SHALL stop all output streams of that connection and MUST NOT terminate any session. A session ends only when it is closed with `close`, when its pane is closed through a workspace operation (see `workspace-model`), when an embedded server shuts down or a daemon's sessions are ended (see `server-modes`), or when its process exits.

#### Scenario: Disconnect leaves sessions running
- **WHEN** a client with two streamed sessions disconnects
- **THEN** both sessions keep running on the server
- **AND** a later client can attach to them by id

#### Scenario: Daemon stop leaves sessions running
- **WHEN** a daemon is stopped with `splitty daemon stop` while a client streams one of its sessions
- **THEN** the session's process keeps running, and a client of the next daemon can attach to the session by the same id

### Requirement: Terminal Resize
On `resize` with a session `id`, `cols` and `rows`, the server SHALL record that size as the size at which the requesting connection shows the session, and SHALL set the session's PTY to the size its PTY size policy derives from the recorded sizes (see PTY Size Policy). A `create` SHALL record its `cols` and `rows` for the creating connection in the same way. The server SHALL answer every `resize` with a `size` event for the session to the requesting connection, carrying the session's PTY size after the report, also when the size did not change. When a `resize` carries `redraw: true` and the PTY size does not change, the server SHALL resize the PTY by one row and back to its size, so the session's processes redraw, without sending a `size` event for the intermediate size. A `resize` with `cols` or `rows` of 0 SHALL change nothing and be answered with an `error` event for the session.

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

### Requirement: Closing a Session
On `close` with a session `id`, the server SHALL stop the connection's output stream for that session, hang up and release the session's terminal, remove the session so it can no longer be attached, and send a `closed` event with that `sessionId` and without `exitCode` to the requesting connection.

#### Scenario: Close an existing session
- **WHEN** a client sends `{"type":"close","id":"<sid>"}`
- **THEN** the client receives a `closed` event for that session without `exitCode`
- **AND** a later `attach` to that id fails with `session not found`

#### Scenario: Close an unknown session
- **WHEN** a client sends `close` with an `id` that names no session
- **THEN** the server sends an `error` event with that `sessionId`

### Requirement: Process Exit
When a session's process exits, the server SHALL send each connection streaming that session a `closed` event with the `sessionId` and the actual `exitCode` when available, and SHALL stop streaming the session after draining its available final output. A nonzero exit code MUST NOT be collapsed to a generic failure value. Signal termination SHALL carry `signal` when available; an unavailable code SHALL be absent rather than falsely zero. The exited session SHALL remain registered until it is closed, including automatic closure according to the owning host policy. The `session.exited` event is a notification and is outside this capability. An ordinary mux pane whose exit matches the owning server close_on_exit setting SHALL be closed after final output has had time to drain, including when no clients are attached.

#### Scenario: Shell exits
- **WHEN** the user types `exit` in a streamed session's shell
- **THEN** the streaming client receives `closed` with `exitCode` 0

#### Scenario: Command fails
- **WHEN** a session created with `restoreMode` `close` runs a command that exits with a failure status
- **THEN** the streaming client receives `closed` with that actual non-zero `exitCode`

#### Scenario: Exited session can still be closed
- **WHEN** a client sends `close` for a session whose process has already exited
- **THEN** the server removes the session and sends `closed`

#### Scenario: Exact managed-process result
- **WHEN** a directly executed job program exits with code 23
- **THEN** its job state and streamed `closed` event both report 23

### Requirement: Session Metadata Updates
While a connection streams a session, the server SHALL send `title` events that describe the session's foreground process: process name, working directory, command line, whether the foreground process is the session's original shell (`baseShell`), and the last title the program set with OSC 0 or OSC 2 (`oscTitle`). The server SHALL send a `title` event when streaming starts if process information is available, whenever this information changes (checked about once per second), and immediately when the program sets a new OSC title. Process name, working directory and command line SHALL be provided on Linux and macOS; on other platforms they MAY be empty. Events SHALL also include displayTitle and busy, using the same values as session snapshots. busy denotes a known foreground process other than the original shell, not agent thinking; exited sessions are not busy. displayTitle uses a non-base-shell OSC title when present, else the process name; known shell names include the working directory shortened under the server home. Display titles SHALL be plain text bounded to 500 characters.

#### Scenario: Foreground program changes
- **WHEN** the user starts `vim` in a streamed session's shell
- **THEN** within about a second the client receives a `title` event whose process name is `vim` and `baseShell` is false

#### Scenario: Program sets a window title
- **WHEN** a program in the session emits an OSC 2 title sequence
- **THEN** the client receives a `title` event carrying that `oscTitle` without waiting for the next periodic check

### Requirement: Continuous Output Capture
The server SHALL read each session's output continuously from the moment the session starts until its process exits, whether or not any connection streams the session. Output produced while no connection streams the session SHALL be appended to the scrollback buffer, title, prompt and terminal mode tracking SHALL keep running, and the session's process MUST NOT stall because no client reads its output.

#### Scenario: Output while nobody streams
- **WHEN** a session whose pane no client has open prints 10 KiB of output
- **THEN** a client that attaches afterwards receives that output in the replay

#### Scenario: Busy process without a client
- **WHEN** a process writes more output than the kernel's PTY buffer holds while no connection streams its session
- **THEN** the process keeps running and the newest output is in the scrollback buffer

#### Scenario: Modes set while nobody streams
- **WHEN** a program in a session that no connection streams enables mouse reporting and then prints more than the scrollback buffer holds
- **THEN** a client that attaches afterwards receives the sequence that enables mouse reporting ahead of the buffered output

### Requirement: Terminal Modes in the Replay
Ahead of a session's buffered output, a replay SHALL carry the sequences that put a fresh terminal into the modes that output starts in (see Tracked Terminal Modes): the alternate screen as it was at the oldest buffered byte, and every other mode as its last change before that byte left it. A mode changed within the buffered output is left to the replay. The kitty keyboard flags SHALL be restored as the normal screen's stack, then the switch to the alternate screen, then the alternate screen's stack. Synchronized output (`?2026`) SHALL NOT be restored.

#### Scenario: Modes set before the buffered output
- **WHEN** a program enabled application cursor keys, bracketed paste, focus reports and mouse tracking with SGR encoding, then printed more than the scrollback buffer holds, and a client attaches
- **THEN** the replay begins with the sequences that enable each of them, ahead of the buffered output

#### Scenario: Alternate screen left within the buffered output
- **WHEN** a program entered the alternate screen with `CSI ? 1049 h`, printed more than the scrollback buffer holds, left the alternate screen with `CSI ? 1049 l` and printed a prompt
- **THEN** a replay begins with `CSI ? 1049 h`, so the buffered output before the switch back is drawn on the alternate screen and the prompt on the normal screen

#### Scenario: Mode changed within the buffered output
- **WHEN** a program enabled bracketed paste before the oldest buffered byte and disabled it within the buffered output
- **THEN** the replay does not enable bracketed paste ahead of the buffered output

#### Scenario: Kitty keyboard flags of both screens
- **WHEN** before the oldest buffered byte a program pushed the kitty keyboard flags 1 on the normal screen, entered the alternate screen with `CSI ? 1049 h` and pushed the flags 5 there
- **THEN** the replay begins with `CSI > 1 u`, `CSI ? 1049 h` and `CSI > 5 u`, in this order

#### Scenario: Synchronized output is not restored
- **WHEN** a program began synchronized output with `CSI ? 2026 h` before the oldest buffered byte and never ended it
- **THEN** the replay does not begin synchronized output

#### Scenario: Replay filtering keeps the restored modes
- **WHEN** a replay that begins with restored modes passes the replay filtering of report requests and clipboard writes, in the server, in a client or in `splitty attach`
- **THEN** the restored sequences pass unchanged

#### Scenario: Session adopted after a daemon restart
- **WHEN** a starting daemon adopts a session whose keeper's buffered output enabled bracketed paste, and the session then prints more than the scrollback buffer holds
- **THEN** a client attaching to the adopted session receives `CSI ? 2004 h` ahead of the buffered output

### Requirement: Tracked Terminal Modes
The server SHALL follow, in each session's output from the first byte it records and with the output offset of each change, these modes: the alternate screen (`?1049`, `?1047`, `?47`), autowrap (`?7`), application cursor keys (`?1`), the application keypad (`ESC =`, `ESC >`, `?66`), cursor visibility (`?25`) and style (`CSI Ps SP q`), bracketed paste (`?2004`), focus reports (`?1004`), mouse tracking (`?9`, `?1000`, `?1002`, `?1003`) and its encoding (`?1005`, `?1006`, `?1015`, `?1016`), alternate scroll (`?1007`), color-scheme reports (`?2031`), in-band size reports (`?2048`), `modifyOtherKeys` (`CSI > 4 ; Pv m`), and the kitty keyboard flags (`CSI > u`, `CSI < u`, `CSI = u`) with one stack per screen. Where terminals differ, the tracking SHALL follow xterm. Bytes inside OSC, DCS, APC, PM and SOS strings SHALL NOT count as sequences, and a sequence split between two reads SHALL count once. A full reset (`ESC c`) SHALL return every tracked mode to a fresh terminal's state, and a soft reset (`CSI ! p`) cursor visibility and style, autowrap, application cursor keys and keypad, and `modifyOtherKeys`.

#### Scenario: Full reset
- **WHEN** a program entered the alternate screen and enabled bracketed paste and mouse tracking, then sent `ESC c`, all before the oldest buffered byte
- **THEN** the replay restores none of these modes

#### Scenario: Soft reset
- **WHEN** a program hid the cursor and enabled bracketed paste, then sent `CSI ! p`, all before the oldest buffered byte
- **THEN** the replay enables bracketed paste and does not hide the cursor

#### Scenario: The last mouse tracking mode counts
- **WHEN** a program enabled mouse tracking with `?1002` and then with `?1003`
- **THEN** the replay enables `?1003` only
- **AND** after a later `CSI ? 1002 l` it enables none, since resetting any tracking mode turns mouse tracking off

#### Scenario: Sequences end where strings end
- **WHEN** a program sent a DCS string whose payload holds `[?2004h`, a window title ended by BEL, and then `CSI ? 25 l`
- **THEN** the replay hides the cursor and does not enable bracketed paste

#### Scenario: Sequence split between reads
- **WHEN** `CSI ? 2004 h` reaches the server in two reads of the terminal, split at any byte
- **THEN** it counts as enabling bracketed paste

#### Scenario: Eight-bit control bytes
- **WHEN** the output holds the byte 0x9b followed by `?2004h`, as inside UTF-8 text
- **THEN** it does not count as enabling bracketed paste

### Requirement: Client Handling of Session Streams
The server streams a session at most once per connection, so a client SHALL keep at most one `attach` for a session in flight on a connection, and SHALL send `detach` for a session only when none of its views shows that session or waits for it. Binary output frames for a session that arrive after the client sent `attach` and before the matching `attached` event are that session's buffered output; the client SHALL display them, in order, before any output that follows. When a view is replaced by another view for the same session while its attach is in flight, the new view SHALL take over that attach and receive its buffered output and all later output. Historical output SHALL NOT perform clipboard operations or produce replies to terminal report requests. Live output after `attached` SHALL retain its normal behavior even when buffered before a view is wired. The server SHALL filter clipboard operations and terminal report requests from replay before sending it over any transport. Clients SHALL retain their replay protection for older servers.

#### Scenario: Buffered output shown on attach
- **WHEN** a client attaches a view to a session whose buffered output ends with a shell prompt
- **THEN** the view shows that prompt before any output produced after the attach

#### Scenario: View replaced while its attach is in flight
- **WHEN** a view's attach is in flight and a new view for the same session replaces it
- **THEN** the client sends that session one `attach` and no `detach`
- **AND** the new view shows the buffered output and keeps receiving new output

#### Scenario: Last view closed
- **WHEN** the only view of a session is closed without closing the session
- **THEN** the client sends `detach` for that session

#### Scenario: Resume with old queries and copies
- **WHEN** historical output includes terminal color/status queries or OSC 52 clipboard operations, including across frame boundaries
- **THEN** restoring the view neither sends their replies into the session nor changes the clipboard
- **AND** live queries and clipboard writes after `attached` behave normally

### Requirement: Image Upload
The server SHALL accept `{"type":"save_image","requestId":"<r>","data":"<base64>"}`, with `data` an image of at most 10 MiB encoded as standard base64. It SHALL store the image as a new file in `splitty/paste/` of the user's cache directory (`$XDG_CACHE_HOME`, else `~/.cache`), the directory with mode 0700 and the file with mode 0600, and answer `{"type":"image_saved","requestId":"<r>","path":"<absolute path>"}`. The server SHALL choose the file's name, `paste-<UTC date and time>-<random hex>` with the extension `.png`, `.jpg`, `.gif` or `.webp` from the image's leading bytes. Data that is not valid base64, is larger than 10 MiB or is not a PNG, JPEG, GIF or WebP image SHALL be answered with `{"type":"image_failed","requestId":"<r>","error":"<message>"}` and stored nowhere. Before it stores an image, the server SHALL remove the `paste-` files of that directory that are older than 24 hours, and then all but the newest 100.

#### Scenario: A screenshot
- **WHEN** a client sends `save_image` with request id `r1` and a PNG image of 200 KiB
- **THEN** the server answers `image_saved` with `requestId` `r1` and a `path` ending in `.png` in `splitty/paste/` of the cache directory, and that file holds the image's bytes and has mode 0600

#### Scenario: Not an image
- **WHEN** a client sends `save_image` whose data decodes to a shell script
- **THEN** the server answers `image_failed` with the request id, and no file is created

#### Scenario: Too large
- **WHEN** a client sends `save_image` whose data decodes to 11 MiB
- **THEN** the server answers `image_failed`, and no file is created

#### Scenario: Old pastes
- **WHEN** the directory holds a `paste-` file from two days ago and a client saves an image
- **THEN** the old file is gone and the new one is stored

### Requirement: Image Paste In A Pane
When the user pastes into a pane and the clipboard data holds an image file and no text, the client SHALL send each image with `save_image` to the server of that pane's connection and SHALL paste the returned path into that pane as a bracketed paste, several paths separated by spaces. Ctrl+V without other modifiers SHALL be pasted this way where the browser pastes with it (not on macOS); when the clipboard then holds neither an image nor another file, the client SHALL send `^V` to the terminal as before. Other files and all dropped images SHALL follow Files Into A Pane. A paste or drop without a file SHALL behave as before. When the server answers `image_failed`, or does not know `save_image`, the client SHALL say so in a notice and paste nothing; for a server that does not know it, the notice SHALL say that it needs an update.

#### Scenario: Screenshot into Claude Code on a host
- **WHEN** the clipboard holds a screenshot and the user presses Ctrl+V in a pane of a remote screen of `vm1`
- **THEN** the image is stored on `vm1`, its path is pasted into that pane as a bracketed paste, and nothing is sent to any other server

#### Scenario: Ctrl+V with text
- **WHEN** the clipboard holds text and the user presses Ctrl+V in a pane
- **THEN** the terminal receives `^V` and no `save_image` is sent

#### Scenario: Host with an older Splitty
- **WHEN** the user pastes an image into a pane of a host whose server answers `save_image` with "unknown message type"
- **THEN** a notice says the host needs an update, and nothing is pasted

### Requirement: Direct Managed Process Launch
A managed job SHALL execute its program with literal individual arguments in its validated working directory under the server's OS user, using a PTY with normal terminal environment variables. It SHALL not launch an implicit login shell, source a shell profile, inject a command after a delay, or continue in a shell after exit. Explicit shell programs in argv SHALL remain permitted.

#### Scenario: Arguments are data
- **WHEN** a job passes `$(touch marker)` and `two words` as arguments to a program that prints its arguments
- **THEN** they arrive as two literal arguments, no marker is created, and the job ends when that program ends

### Requirement: Job Output Does Not Depend On Attach
A managed session SHALL expose retained raw output and incremental sanitized log text through independent bounded reads from its single continuous capture. Reads SHALL not take over the PTY reader or use an unbounded stream queue. Text conversion SHALL have bounded parser state, retain partial UTF-8 correctly and never execute terminal commands.

#### Scenario: No UI attached and large output
- **WHEN** a job emits more than its retention size without an attached UI
- **THEN** it continues running, bounded reads return the retained suffix with loss information, and a later UI attach still streams normally

### Requirement: Controlled Managed Job Stop
A managed stop SHALL request SIGTERM for the still-owned job process and controlled PTY process groups, wait up to 3 seconds, then use SIGKILL if needed and await termination. It SHALL preserve output for later reads. It SHALL not signal a reused PID or claim to contain processes that deliberately escaped the managed terminal/process groups. Failure to confirm termination SHALL report an error, not success.

#### Scenario: Process ignores SIGTERM
- **WHEN** a controlled test job ignores SIGTERM
- **THEN** stop escalates after the grace period, confirms termination, and preserves its captured output

#### Scenario: Process already ended
- **WHEN** stop arrives after the job process was reaped
- **THEN** it returns the known result without signaling the former PID or changing another process

### Requirement: Managed Pane Creation Cannot Fall Back
A session `create` for a pane belonging to a managed job SHALL attach to its existing started session or report its pending/terminal outcome. It SHALL never create an independent shell or rerun the command when the job's session is absent, failed or interrupted. This SHALL hold even if a client uses the legacy create fallback.

#### Scenario: Reload a failed or interrupted job
- **WHEN** a client sends `create` for a job pane whose process did not start or was interrupted by server restart
- **THEN** the server reports that state and starts no process

### Requirement: Available Host CLI In Session PATH
When the server is itself the Splitty CLI or has an executable sibling CLI, newly spawned ordinary sessions and managed jobs SHALL receive a PATH that makes that CLI available as `splitty`, through the stable CLI directory of Host CLI Follows The Daemon while its link names that same CLI, otherwise through the CLI's own directory. Its directory SHALL occur once ahead of inherited entries; the relative order of other entries SHALL remain unchanged. This SHALL apply to Windows-hosted sessions inside WSL and remote-host sessions without requiring a global installation or credential injection.

#### Scenario: Bundled WSL backend outside PATH
- **WHEN** the desktop backend runs from a version-specific directory absent from its inherited PATH and starts a session
- **THEN** `splitty agent guide` and `splitty --version` resolve to its available CLI without a process-specific executable path

#### Scenario: Direct managed program inherits the CLI
- **WHEN** a grant-owned managed program is launched with an inherited PATH that lacks Splitty
- **THEN** its subprocesses can locate the available host CLI and receive no owner socket or reusable credential as a consequence

#### Scenario: A daemon's session gets the stable directory
- **WHEN** a daemon whose run directory's `bin/splitty` names its own CLI starts a pane's shell
- **THEN** the shell's PATH starts with that `bin` directory and `SPLITTY_CLI_DIR` names it

### Requirement: CLI Availability After Shell Startup
Splitty's supported interactive Bash/Zsh integration SHALL preserve available host-CLI discovery after normal startup files run, including when those files replace PATH. It SHALL neither edit user startup files nor duplicate the CLI directory on repeated hooks. Already-running session environments SHALL remain untouched.

#### Scenario: Login configuration resets PATH
- **WHEN** a new integrated Bash or Zsh shell loads user startup files that reset PATH
- **THEN** `command -v splitty` still finds the available host CLI when its prompt is ready and unrelated startup behavior is preserved

#### Scenario: Existing session is left alone
- **WHEN** a newer Splitty backend or CLI becomes available
- **THEN** existing shells receive no injected PATH-changing command; the new launch behavior applies to subsequently created sessions

### Requirement: Missing Host CLI Does Not Create A False Command
When an embedded GUI has no available sibling CLI and no usable CLI in its existing PATH, it SHALL not expose the GUI executable as `splitty`, install a wrapper or alter files to simulate availability. Terminal creation SHALL continue normally and documentation SHALL identify the need for an installed or bundled CLI. Missing or non-executable candidates SHALL not add a dead PATH entry.

#### Scenario: GUI-only installation
- **WHEN** an embedded GUI starts a session without any installed or bundled CLI
- **THEN** the session starts normally without a misleading `splitty` command or filesystem installation

### Requirement: Host CLI Follows The Daemon
A starting daemon SHALL point `bin/splitty` in its run directory (`~/.local/share/splitty/run/`, or `run/` of `SPLITTY_CONFIG_DIR`) at its own CLI before it adopts keepers or starts sessions, replacing an earlier link without a moment in which the name is missing, and SHALL leave the link in place when it stops. When it cannot, it SHALL log why in `daemon.log` and its sessions SHALL get the CLI's own directory.

#### Scenario: An update gives a kept shell the new CLI
- **WHEN** a pane's shell was started by a daemon of one installation, that daemon stops with its sessions kept, and a daemon of another installation starts
- **THEN** `splitty` in the still running shell runs the other installation's CLI

#### Scenario: Another installation's link is not handed out
- **WHEN** `bin/splitty` of the run directory names the CLI of another installation than the server that starts a session
- **THEN** the session's PATH gets that server's own CLI directory instead

### Requirement: PTY Size Policy
A session's PTY size SHALL be the smallest columns and rows, taken separately, of the sizes recorded by the connections its policy chooses: `active` (default) the active ones, or as `latest` if none is; `latest` the one with the latest activity, or all if none had one; `smallest` all. A connection is active while its latest activity is within the idle time and it has not become idle since. Without recorded sizes the PTY size SHALL stay.

#### Scenario: The smaller active client decides
- **WHEN** clients A (120×40) and B (100×30) show a session under `active` and both had an activity within the idle time
- **THEN** the session's PTY size is 100 columns and 30 rows

#### Scenario: An idle client drops out
- **WHEN** clients A (120×40) and B (100×30) show a session under `active`, and B's last activity is older than the idle time while A's is not
- **THEN** the session's PTY size becomes 120 columns and 40 rows without any client message

#### Scenario: Nobody active
- **WHEN** no client showing a session under `active` is active, and B had the latest activity
- **THEN** the session's PTY size is B's recorded size

#### Scenario: Latest activity decides
- **WHEN** clients A (120×40) and B (100×30) show a session under `latest` and A had the latest activity
- **THEN** the session's PTY size is 120 columns and 40 rows

#### Scenario: Smallest ignores activity
- **WHEN** clients A (120×40) and B (100×30) show a session under `smallest` and B never had an activity
- **THEN** the session's PTY size is 100 columns and 30 rows

#### Scenario: A client that leaves gives the size back
- **WHEN** clients A (120×40) and B (100×30) show a session under `smallest` and B disconnects
- **THEN** the session's PTY size becomes 120 columns and 40 rows

### Requirement: Size Reports of Older Clients
A connection that has never sent `activity` or `idle`, a client that predates the PTY size policy, SHALL have an activity at each of its `resize` messages.

#### Scenario: Older clients count by their resizes
- **WHEN** two connections that never sent `activity` or `idle` show a session under `latest`, and the second one sends `resize` with 90×25 after the first one's 120×40
- **THEN** the session's PTY size is 90 columns and 25 rows

### Requirement: PTY Size Policy Settings
The server SHALL read the policy and the idle time from its settings, `terminal.size_policy` (`active`, `latest` or `smallest`, default `active`) and `terminal.size_idle_seconds` (from 10 to 3600, default 120); an invalid value SHALL fall back to the default. An update of the settings SHALL apply to every session at once. The policy SHALL be that of the server that runs the session; a client's settings SHALL NOT change the policy of another server's sessions.

#### Scenario: Policy changed in the settings
- **WHEN** `terminal.size_policy` is changed from `smallest` to `latest` through the settings API while A (120×40, latest activity) and B (100×30) show a session
- **THEN** the session's PTY size becomes 120 columns and 40 rows
- **AND** both clients receive a `size` event with `policy` `latest`

#### Scenario: Invalid value
- **WHEN** the server's settings name `terminal.size_policy = "largest"` and `terminal.size_idle_seconds = 5`
- **THEN** the server uses `active` and 120 seconds

### Requirement: Client Activity
A connection SHALL tell the server when its user acts and when it is not in use: `activity` records an activity of the connection at the time of the message; `activity` with `takeover: true` additionally makes every other connection of the server idle until its next `activity`; `idle` makes the connection idle until its next `activity`. Each SHALL apply the PTY size policy again to the server's sessions.

#### Scenario: Takeover
- **WHEN** clients A (120×40) and B (100×30) show a session under `active`, both active, and A sends `{"type":"activity","takeover":true}`
- **THEN** the session's PTY size becomes 120 columns and 40 rows
- **AND** it becomes 100 columns and 30 rows again once B sends `activity`

#### Scenario: Hidden window
- **WHEN** clients A (120×40) and B (100×30) show a session under `active`, both active, and B sends `idle`
- **THEN** the session's PTY size becomes 120 columns and 40 rows

### Requirement: Withdrawn Size Reports
`hide` with a session `id` SHALL remove the connection's recorded size for that session until its next `resize` for it. Detaching, closing a session and disconnecting SHALL remove the connection's recorded sizes for the sessions concerned. Each SHALL apply the PTY size policy again to the affected sessions.

#### Scenario: Session not shown
- **WHEN** clients A (120×40) and B (100×30) show a session under `smallest` and B sends `hide` for it
- **THEN** the session's PTY size becomes 120 columns and 40 rows

#### Scenario: Detached session
- **WHEN** clients A (120×40) and B (100×30) show a session under `smallest` and B sends `detach` for it
- **THEN** the session's PTY size becomes 120 columns and 40 rows

### Requirement: PTY Size Capability
The server SHALL announce `pty-size-v1` in `mux.welcome.capabilities` when it supports size reports with `redraw`, `activity`, `idle`, `hide` and the `size` event.

#### Scenario: Capability announced
- **WHEN** a client connects
- **THEN** `mux.welcome.capabilities` contains `pty-size-v1`

### Requirement: PTY Size Events
The server SHALL send `size` with the `sessionId`, `cols`, `rows` and the `policy` in effect to every connection that streams a session whenever the session's PTY size set by the policy changes and whenever the policy is reconfigured. On `attach`, when the session has a PTY size set by the policy, the server SHALL send `size` before the buffered output; after `create`, it SHALL send the session's size once one is set.

#### Scenario: Other clients learn the new size
- **WHEN** clients A and B stream a session and A's `resize` changes its PTY size to 100×30
- **THEN** B receives a `size` event with `cols` 100 and `rows` 30

#### Scenario: Size before the replay
- **WHEN** a client attaches to a session whose PTY size was set to 100×30 by the policy
- **THEN** it receives the `size` event before the first binary frame of the replay

### Requirement: Client Size Reports
With a server that announces `pty-size-v1`, a client SHALL report as `resize` only the size of a shown pane (its panel visible on the active screen), report it again when the pane becomes shown, and send `hide` when it stops being shown. Its first report after an attach SHALL carry `redraw: true` instead of resizing the PTY by one row and back. With other servers it SHALL resize the PTY as before and send none of these messages.

#### Scenario: Background screen
- **WHEN** the user switches from a host's screen to another screen of the same host
- **THEN** the client sends `hide` for the sessions of the screen it left and `resize` for those of the screen it shows

#### Scenario: Older server
- **WHEN** a client attaches to a session of a server whose `mux.welcome` has no `pty-size-v1`
- **THEN** it resizes the PTY by one row and back after the attach and sends no `activity`, `idle` or `hide`

### Requirement: Client Activity Reports
A client SHALL send `activity` to a server that announces `pty-size-v1` only for its user's own interaction with the window and only to the connection of the active screen, at least once per idle time while the user interacts. It SHALL send `idle` to those connections when the window becomes hidden, and on each `mux.welcome` with the capability `activity` or `idle`, before it attaches any session, according to whether its window is visible and focused on a screen of that connection.

#### Scenario: Automatic actions do not take the size
- **WHEN** a client reconnects and re-attaches the sessions of a screen while its window is not focused
- **THEN** it sends `idle` before attaching and no `activity`
- **AND** the sessions' PTY sizes stay those of the client in use

### Requirement: Client Display of the PTY Size
A client SHALL show each session at the PTY size of the latest `size` event. When that size differs from the pane's, it SHALL show that the size is set by another client and, unless the policy is `smallest`, SHALL offer to take the size over with `activity` and `takeover: true`. A session taller than the pane SHALL keep the row of its cursor visible. A `size` event outside 1–1000 columns or 1–500 rows, or with an unknown policy, SHALL be ignored.

#### Scenario: Narrower PTY
- **WHEN** a pane fits 120 columns and the session's `size` event says 100 columns
- **THEN** the client shows the session in 100 columns and marks the remaining width as set by another client

#### Scenario: Oversized event
- **WHEN** a host sends a `size` event with 5000 columns
- **THEN** the client ignores it and keeps showing the session at the previous size

### Requirement: Chunked File Uploads
The server SHALL accept `{"type":"upload","requestId":"<r>","offset":<n>,"data":"<base64>"}` with at most 1 MiB of decoded data per message, the first message of an upload (`offset` 0) also carrying `name` and `size`, the file's length in bytes, at most 512 MiB. It SHALL answer every message with `upload_progress` (`requestId`, `received`) while bytes are missing, with `upload_saved` (`requestId`, `path`, an absolute path) once `received` equals `size`, or with `upload_failed` (`requestId`, `error`). By default, each upload SHALL be stored as a new file with mode 0600, under the name of D4 of the design (the last component of `name`, characters other than letters, digits, `.`, `-` and `_` replaced by `_`, no leading dot, at most 120 bytes, else `file`), in a new directory `upload-<UTC date and time>-<random hex>` of the image store's directory (`splitty/paste/` of the cache directory, mode 0700). A message whose `offset` differs from the bytes received, that would exceed `size`, that is not a first message for an unknown `requestId`, or that would start a fifth concurrent upload of the connection SHALL fail the upload; a failed upload and every unfinished upload of a connection that closes SHALL be removed. Before it starts an upload, the server SHALL remove the `upload-` directories of that directory that are older than 24 hours, and then all but the newest 100.

Servers SHALL announce `upload-control-v1` for destination selection and cancellation. The first `upload` chunk MAY carry `dir`, an existing absolute directory. Such uploads SHALL retain the sanitized filename, use mode 0600, appear at the destination only when complete, and never replace an existing entry. The directory itself SHALL NOT be created or have its permissions changed. `upload_cancel` with `requestId` SHALL discard that connection's unfinished upload; already completed files SHALL remain. Cancellation SHALL be idempotent.

#### Scenario: Upload in two chunks
- **WHEN** a client uploads `notes.txt` of 1.5 MiB in two messages
- **THEN** the server answers the first with `upload_progress` and `received` 1 MiB, the second with `upload_saved` whose `path` ends in `/notes.txt` inside a new `upload-` directory, and that file holds the bytes and has mode 0600

#### Scenario: Name with spaces and a path
- **WHEN** the first message names `../My Report (final).pdf`
- **THEN** the file is stored as `My_Report__final_.pdf`

#### Scenario: Chunk out of order
- **WHEN** after a first chunk of 1 MiB the client sends a chunk with `offset` 0
- **THEN** the server answers `upload_failed`, and nothing of the upload remains

#### Scenario: Connection lost during an upload
- **WHEN** a connection closes while one of its uploads has received half of its file
- **THEN** that upload's directory is removed

#### Scenario: Durable upload
- **WHEN** a client uploads a file with an existing absolute `dir`
- **THEN** the completed file appears directly in that directory, outside temporary retention, and no partial destination file is visible

#### Scenario: Existing destination
- **WHEN** an entry already exists at the chosen destination filename
- **THEN** the upload fails without replacing that entry or leaving incomplete data

#### Scenario: Cancel unfinished upload
- **WHEN** a client sends `upload_cancel` for its unfinished upload
- **THEN** that upload's incomplete data is removed and other uploads and completed files remain

### Requirement: Files Into A Pane
When the user pastes into a pane and the clipboard data holds files and no text, the client SHALL send each image (PNG, JPEG, GIF, WebP) of at most 10 MiB with `save_image` and every other file, larger images included, with chunked `upload` messages to the server of that pane's connection, sending the next chunk only after the answer to the last, and SHALL paste the returned paths into that pane as one bracketed paste, in the order of the files and separated by spaces. Files over 512 MiB SHALL be refused before anything is sent. When the server answers `upload_failed`, or does not know `upload`, the client SHALL say so in a notice and paste nothing; for a server that does not know it, the notice SHALL say that it needs an update.

Dropped files SHALL wait for the user's choice before any bytes are sent. Temporary storage with path insertion SHALL be the default; an existing absolute directory and optional path insertion SHALL be the alternative. The client SHALL show the target server, files, total size and destination; it SHALL suggest only a known absolute session working directory. All confirmed dropped files, including images, SHALL use chunked uploads. The pane SHALL show aggregate byte and file progress with cancellation. Cancellation SHALL stop further chunks and files and request removal of the unfinished upload; completed files remain. Failed or cancelled batches SHALL insert no paths. Inserted paths SHALL be quoted when necessary to remain literal shell arguments. Older hosts without `upload-control-v1` SHALL receive no dropped file bytes and the user SHALL be told to update.

#### Scenario: PDF into Claude Code on a host
- **WHEN** the user drops `spec.pdf` and confirms temporary storage onto a pane of a remote screen of `vm1`
- **THEN** the file is stored on `vm1`, its path is pasted into that pane, and nothing is sent to any other server

#### Scenario: Host with an older Splitty
- **WHEN** the user confirms a dropped text file in a pane of a host whose server answers `upload` with "unknown message type"
- **THEN** a notice says the host needs an update, and nothing is pasted

#### Scenario: Cancel destination selection
- **WHEN** the user drops files and dismisses the choice
- **THEN** no upload or image message is sent and nothing is inserted

#### Scenario: Optional insertion
- **WHEN** the user uploads to a directory with insertion disabled
- **THEN** the file is stored there and no path is inserted

#### Scenario: Cancel a batch
- **WHEN** the user cancels while the second file is transferring
- **THEN** no third file starts, the second file's incomplete data is discarded, the first file remains, and nothing is inserted

### Requirement: File Offers
The server SHALL announce `file-offer-v1` in `mux.welcome.capabilities` and SHALL send a file offer (see `control-api`, File Offer Requests) as `{"type":"file.offer","offerId","sessionId","name","size","expiresAt"}` to every connection that sent `file.register` after that welcome and is attached to the offer's session. The first `file.accept` naming the offer SHALL take it, and the server SHALL send `{"type":"file.closed","offerId","reason":"taken"}` to the other recipients. Only the accepting connection SHALL read it, with `{"type":"file.read","offerId","offset"}` at the next offset, answered by `{"type":"file.chunk","offerId","offset","data","eof"}` with at most 1 MiB of the file opened at offer time, as base64, `eof` true on the last chunk. `file.decline` SHALL decline the offer once every recipient declined or went away; from the accepting connection it SHALL end the transfer as declined. The server SHALL close an offer not accepted within 60 seconds (`expired`) and an accepted one whose reader sends nothing for 60 seconds or disconnects (`disconnected`), and one whose file cannot be read to its end (`failed`), and send `file.closed` with that reason to the recipients still connected.

#### Scenario: Two windows show the session
- **WHEN** two registered connections are attached to a session that offers a file, and the second accepts
- **THEN** the first receives `file.closed` with reason `taken`, and only the second may read the file

#### Scenario: Reading past the order
- **WHEN** the accepting connection sends `file.read` with an offset other than the next one
- **THEN** the server sends no chunk of that file for it

### Requirement: Saving An Offered File
A client SHALL register for file offers on connections whose welcome announced `file-offer-v1`, and SHALL show each valid offer as a notice that names the file as plain text, its size and its server (the host's name, or this computer), with Save and Dismiss. It SHALL send nothing but `file.decline` before the user chose Save, SHALL show at most 3 offers at once, and SHALL decline malformed offers and offers beyond that limit. On Save it SHALL accept the offer, read it chunk by chunk, show progress with a way to cancel, which sends `file.decline`, and save the file under its name as a download of the window once the last chunk arrived; on `file.closed` it SHALL drop the notice. In a desktop shell, the notice that the file was saved SHALL offer to show it in the system file manager, which asks the shell to show that download (`access-control`, Showing A Saved Download); when the shell cannot, the window SHALL say so. In a browser the notice SHALL offer no such action.

#### Scenario: Save from a host
- **WHEN** a host offers `report.pdf` and the user chooses Save
- **THEN** the window reads the file from that host's connection only and saves `report.pdf` as a download

#### Scenario: Dismiss
- **WHEN** the user dismisses an offer
- **THEN** the client sends `file.decline` and reads nothing

#### Scenario: Show the saved file
- **WHEN** the desktop app saved `report.pdf` from an offer and the user chooses to show it
- **THEN** the window asks its shell to show the download it just saved, and the file manager opens with `report.pdf` selected

### Requirement: Byte-Preserving Terminal Input
A server announcing `terminal-input-bytes-v1` SHALL accept `input_bytes` with a session `id` and standard padded base64 `data`, decode at most 64 KiB and write those exact bytes using the same session authority, order and write-failure behavior as `input`. Existing `input` remains unchanged. Payload shapes are in `docs/protocol.ts`.

#### Scenario: Binary mouse report and text remain ordered
- **WHEN** an authorized client sends `input`, then `input_bytes` containing bytes invalid as UTF-8, then another `input` for an existing session
- **THEN** the process receives their exact bytes in that order without text conversion or interleaving within a request
- **AND** the connection need not stream that session

#### Scenario: Invalid or excessive encoded input
- **WHEN** `input_bytes` has invalid base64 or decodes to more than 64 KiB
- **THEN** the server sends an error for that session and writes no part of the request
- **AND** encoded length is bounded before decoding and decoded storage remains charged through writing

#### Scenario: Byte input for an unknown session
- **WHEN** valid `input_bytes` names no session
- **THEN** the server sends an `error` with that `sessionId` and `session not found`

### Requirement: Compatible Native Byte Input
A native client SHALL preserve terminal input bytes in one bounded request and send `input_bytes` only after its current connection announces `terminal-input-bytes-v1`. With an older server it SHALL send valid UTF-8 through `input`, reject non-UTF-8 input visibly and never perform lossy conversion. Attachment, generation, replay and no-retry rules SHALL apply equally to both forms.

#### Scenario: Older server
- **WHEN** the current connection lacks the capability
- **THEN** UTF-8 keyboard input remains usable and non-UTF-8 reports are refused without sending corrupt bytes

#### Scenario: Unattached or obsolete terminal
- **WHEN** raw input belongs to a terminal still attaching or to an obsolete pane/session generation
- **THEN** it is rejected rather than delivered to another terminal or replayed later

### Requirement: Native Local Terminal Search
The native client SHALL search literal text across the active terminal's retained screen and history, folding ASCII case and matching other bytes exactly. Soft line wraps SHALL NOT split searchable text. Search navigation SHALL wrap through matches without changing clipboard selection or sending query text, search keys or search-control pointer gestures to the session.

#### Scenario: Retained output and Unicode
- **WHEN** a query matches retained output across a soft wrap, including a wide or combining character
- **THEN** matching and highlights use the corresponding terminal cells, and navigation can reveal matches outside the current viewport
- **AND** ASCII case differences match while non-ASCII case differences are not folded

#### Scenario: Search input remains local
- **WHEN** the user edits, navigates or closes native terminal search while a terminal application requests mouse input
- **THEN** those interactions do not reach the session and existing clipboard selection remains independent

### Requirement: Native Search Progress And Limits
Native search SHALL distinguish incomplete indexing from completed results. Exceeding its query, match or storage limit SHALL discard results and report a visible limit state instead of presenting a partial search as complete. Only the focused visible pane SHALL retain an active index; query text SHALL remain local to its terminal across deactivation. Terminal reset SHALL invalidate old results.

#### Scenario: Broad query exceeds a limit
- **WHEN** a search exceeds a resource limit
- **THEN** it stops with a visible refine-query state, leaving the terminal usable
- **AND** output alone does not retry the failing query; changing the query can start a new full-history search

#### Scenario: Pane activation and new output
- **WHEN** a pane with an open search that has not failed becomes inactive and later active again
- **THEN** its query remains available and results are rebuilt from current retained output
- **AND** counts and highlights from an old index or terminal generation are not reused

#### Scenario: Output and resize invalidate results
- **WHEN** output, a screen-buffer switch or a resize changes a searched terminal
- **THEN** search refreshes against the current terminal content before presenting updated results, retaining a valid navigation anchor where supported
- **AND** indexing progress is not reported as completed prematurely

### Requirement: Deliberate Native Terminal Links
The native terminal SHALL request the system browser to open an absolute HTTP(S) target only after a deliberate Ctrl/Cmd-left-click without Shift. Retained OSC 8 targets SHALL take precedence over detected text, including when that target is rejected. Output, hover and ordinary selection SHALL NOT open a target. Generic link activation SHALL NOT dispatch file, editor or other URI schemes.

#### Scenario: Explicit target differs from visible text
- **WHEN** a terminal cell has an allowed OSC 8 target different from its visible label
- **THEN** link feedback and deliberate activation use the actual target
- **AND** an invalid or disallowed retained explicit target does not fall back to a URL in its label

#### Scenario: Detected text in retained output
- **WHEN** a complete absolute HTTP(S) URL occurs in retained terminal text across a soft wrap, including Unicode or wide characters
- **THEN** deliberate activation opens that complete detected target
- **AND** hard line boundaries and adjacent punctuation do not become part of the target

### Requirement: Current And Bounded Native Link Activation
Native link resolution SHALL bound inspected output and target storage, reject malformed or incomplete targets and never open a truncated prefix. Activation SHALL validate the current target again at gesture completion. Dragging, release outside the captured cell, changed target, changed view geometry or session, and lifecycle cancellation SHALL prevent opening. An admitted link gesture SHALL remain local to the UI throughout its lifetime.

#### Scenario: Target changes during a gesture
- **WHEN** output changes the pressed target before release, or the view scrolls, resizes, hides or loses focus
- **THEN** release opens nothing and no part of the captured link gesture reaches the session

#### Scenario: Excessive or malformed target
- **WHEN** a retained explicit target is malformed or exceeds activation limits, or a detected logical line cannot be completely inspected within its limits
- **THEN** activation opens nothing, without substituting a shortened target
- **AND** the terminal remains usable

#### Scenario: Existing mouse and selection behavior
- **WHEN** the user performs an ordinary click or Shift selection while a terminal application requests mouse input
- **THEN** existing application mouse and local selection routes remain available
- **AND** deliberate link activation sends neither mouse bytes nor link text to the session

#### Scenario: Oversized terminal control sequence is discarded
- **WHEN** a hyperlink control sequence exceeds the terminal parser's capture limit and is discarded before creating cell metadata
- **THEN** no shortened explicit target is exposed
- **AND** subsequent visible text follows ordinary URL detection, including a complete URL printed as its label

### Requirement: Default Shell Profile
The system default profile SHALL be the detected profile for the server's login shell (`$SHELL`), or the first detected profile when `$SHELL` names no executable. An empty `[shells] default_profile_id` in `config.toml` SHALL mean that the default follows the system default, resolved again whenever the profiles are reconciled; a non-empty value is an explicit choice. Reconciling profiles with detection MUST NOT write a resolved id into an empty value. `GET /api/shell-profiles` SHALL report the effective `defaultProfileId`, `followSystemDefault` (true exactly when the stored value is empty) and `systemDefaultProfileId`. `PUT /api/shell-profiles` with `followSystemDefault` true SHALL store the empty value; otherwise `defaultProfileId` SHALL be stored as an explicit choice, replaced by the first enabled and available profile when it names none.

#### Scenario: New configuration follows the login shell
- **WHEN** the server starts without a stored default and `$SHELL` is `/usr/bin/zsh`
- **THEN** `GET /api/shell-profiles` reports the zsh profile as `defaultProfileId` and `systemDefaultProfileId` and `followSystemDefault` true
- **AND** `config.toml` keeps `default_profile_id` empty

#### Scenario: Login shell changes
- **WHEN** the default follows the system default and the server starts again with `$SHELL` set to `/bin/bash`
- **THEN** the default profile is the bash profile

#### Scenario: Explicit choice survives a login shell change
- **WHEN** a client saves the shell profiles with `defaultProfileId` naming the bash profile and no `followSystemDefault`
- **AND** the server starts again with `$SHELL` set to `/usr/bin/zsh`
- **THEN** the default profile is still the bash profile and `followSystemDefault` is false

#### Scenario: Switching back to the system default
- **WHEN** a client saves the shell profiles with `followSystemDefault` true
- **THEN** `config.toml` stores an empty `default_profile_id`
- **AND** the response reports the system default profile as `defaultProfileId`
