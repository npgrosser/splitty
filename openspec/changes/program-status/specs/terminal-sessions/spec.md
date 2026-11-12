# Spec Delta

## MODIFIED Requirements

### Requirement: Terminal Input
The server SHALL write the UTF-8 bytes of the `data` field of an `input` message unchanged to the PTY of the session named by `id`. Any connection MAY send input to any existing session; it does not need to be streaming that session. Each input SHALL be written whole: bytes the server writes to a session on its own, such as the answer to a status probe (see `program-status`), come before or after it and never inside it.

#### Scenario: Input reaches the process
- **WHEN** a client sends `{"type":"input","id":"<sid>","data":"ls\r"}`
- **THEN** the bytes `ls\r` are written to that session's terminal

#### Scenario: Input for an unknown session
- **WHEN** a client sends `input` with an `id` that names no session
- **THEN** the server sends an `error` event with that `sessionId` and the message `session not found`

#### Scenario: Write failure
- **WHEN** writing to the session's terminal fails
- **THEN** the server sends an `error` event with the `sessionId` describing the write failure

#### Scenario: Status probe answered during input
- **WHEN** a client's input of 64 KiB is being written to a session while the server answers a status probe in it
- **THEN** the process reads the answer before or after those bytes, not between two of them

### Requirement: Terminal Output Streaming
The server SHALL deliver session output only to connections currently streaming that session, meaning connections that created it with `create` or attached it with `attach` and have not since detached, closed or disconnected. Output MUST be sent as binary output frames as defined in `mux-protocol` (Binary Terminal Output Frames), carrying the raw bytes exactly as read from the terminal, except status probes, which the server answers itself and removes before it buffers or sends the output (see `program-status`, Probe Removed from the Output).

#### Scenario: Output delivered as a binary frame
- **WHEN** the process in session `abc` writes `hello` to its terminal while a client streams `abc`
- **THEN** that client receives a binary output frame for session `abc` whose payload is `hello`

#### Scenario: Non-streaming connections receive nothing
- **WHEN** a session produces output
- **THEN** connections that have not created or attached that session receive no output frames for it

#### Scenario: Status probe removed
- **WHEN** the process in a streamed session writes `a`, `ESC ] 7501 ; ? BEL` and `b`
- **THEN** the client receives `ab` and no part of the probe

### Requirement: Continuous Output Capture
The server SHALL read each session's output continuously from the moment the session starts until its process exits, whether or not any connection streams the session. Output produced while no connection streams the session SHALL be appended to the scrollback buffer, title, prompt, terminal mode and program status tracking SHALL keep running, status probes SHALL be answered, and the session's process MUST NOT stall because no client reads its output.

#### Scenario: Output while nobody streams
- **WHEN** a session whose pane no client has open prints 10 KiB of output
- **THEN** a client that attaches afterwards receives that output in the replay

#### Scenario: Busy process without a client
- **WHEN** a process writes more output than the kernel's PTY buffer holds while no connection streams its session
- **THEN** the process keeps running and the newest output is in the scrollback buffer

#### Scenario: Modes set while nobody streams
- **WHEN** a program in a session that no connection streams enables mouse reporting and then prints more than the scrollback buffer holds
- **THEN** a client that attaches afterwards receives the sequence that enables mouse reporting ahead of the buffered output

#### Scenario: Status while nobody streams
- **WHEN** a program in a session that no connection streams writes a status probe and then reports `blocked`
- **THEN** the program reads the probe's answer, and a client that connects afterwards receives the `blocked` record right after `mux.welcome`

### Requirement: Client Handling of Session Streams
The server streams a session at most once per connection, so a client SHALL keep at most one `attach` for a session in flight on a connection, and SHALL send `detach` for a session only when none of its views shows that session or waits for it. Binary output frames for a session that arrive after the client sent `attach` and before the matching `attached` event are that session's buffered output; the client SHALL display them, in order, before any output that follows. When a view is replaced by another view for the same session while its attach is in flight, the new view SHALL take over that attach and receive its buffered output and all later output. Historical output SHALL NOT perform clipboard operations or produce replies to terminal report requests, the status probe `OSC 7501 ; ?` among them. Live output after `attached` SHALL retain its normal behavior even when buffered before a view is wired. The server SHALL filter clipboard operations and terminal report requests from replay before sending it over any transport. Clients SHALL retain their replay protection for older servers.

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

#### Scenario: Status probe in a replay from an older server
- **WHEN** the replay of an older server, which forwards status probes, holds `ESC ] 7501 ; ? BEL`
- **THEN** neither the web client nor `splitty attach` passes the probe on to a terminal that could answer it
