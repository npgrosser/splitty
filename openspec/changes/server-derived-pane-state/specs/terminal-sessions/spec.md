## MODIFIED Requirements

### Requirement: Session Metadata Updates
While a connection streams a session, the server SHALL send `title` events that describe the session's foreground process: process name, working directory, command line, whether the foreground process is the session's original shell (`baseShell`), and the last title the program set with OSC 0 or OSC 2 (`oscTitle`). The server SHALL send a `title` event when streaming starts if process information is available, whenever this information changes (checked about once per second), and immediately when the program sets a new OSC title. Process name, working directory and command line SHALL be provided on Linux and macOS; on other platforms they MAY be empty. Events SHALL also include displayTitle and busy, using the same values as session snapshots. busy denotes a known foreground process other than the original shell, not agent thinking; exited sessions are not busy. displayTitle uses a non-base-shell OSC title when present, else the process name; known shell names include the working directory shortened under the server home. Display titles SHALL be plain text bounded to 500 characters.

#### Scenario: Foreground program changes
- **WHEN** the user starts `vim` in a streamed session's shell
- **THEN** within about a second the client receives a `title` event whose process name is `vim` and `baseShell` is false

#### Scenario: Program sets a window title
- **WHEN** a program in the session emits an OSC 2 title sequence
- **THEN** the client receives a `title` event carrying that `oscTitle` without waiting for the next periodic check

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
