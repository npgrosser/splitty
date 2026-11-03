# Spec Delta

## MODIFIED Requirements

### Requirement: Terminal Input
The server SHALL write the UTF-8 bytes of the `data` field of an `input` message unchanged to the PTY of the session named by `id`. The sender MUST hold current session control and supply its current generation. A missing, stale or foreign control generation SHALL produce an error without writing any bytes. Legacy writers SHALL receive an upgrade-required error. This governs session-protocol input; independently authorized control API sends retain their existing semantics.

#### Scenario: Input reaches the process
- **WHEN** a controlling client sends `input` with its current generation, session ID and `data` equal to `ls\r`
- **THEN** the bytes `ls\r` are written to that session's terminal

#### Scenario: Input for an unknown session
- **WHEN** a client sends `input` with an `id` that names no session
- **THEN** the server sends an `error` event with that `sessionId` and the message `session not found`

#### Scenario: Write failure
- **WHEN** writing to the session's terminal fails
- **THEN** the server sends an `error` event with the `sessionId` describing the write failure

#### Scenario: Observer or stale input
- **WHEN** an observer, legacy writer or former controller sends input without current control
- **THEN** the server rejects it and writes no bytes to the PTY

#### Scenario: Takeover races with an input write
- **WHEN** a takeover is accepted while the previous controller has a pending input write
- **THEN** that write either completes before takeover is acknowledged or is rejected, and cannot be applied after the acknowledgement


### Requirement: Terminal Resize
On `resize` with a session `id`, valid `cols` and `rows`, the server SHALL set the session's PTY to that size only if the sender holds control and supplies its current generation. Invalid dimensions, missing control, stale generations and foreign controllers SHALL produce an error without changing the PTY. Legacy writers SHALL receive an upgrade-required error. Accepted size changes SHALL be reported to session subscribers through `control.state`.

#### Scenario: Resize applies to the PTY
- **WHEN** the controlling client sends `resize` with its current generation and size 100x30
- **THEN** the session's terminal reports 100 columns and 30 rows to its processes

#### Scenario: Resize of an unknown session
- **WHEN** a client sends `resize` with an `id` that names no session
- **THEN** the server sends an `error` event with that `sessionId` and the message `session not found`

#### Scenario: Observer resize
- **WHEN** an observer changes its viewport and sends a resize, or a former controller sends a delayed resize
- **THEN** the server rejects it and the PTY dimensions remain unchanged

#### Scenario: Invalid dimensions
- **WHEN** the controller requests zero or out-of-range terminal dimensions
- **THEN** the server rejects the request without changing the PTY


### Requirement: Client Handling of Session Streams
The server streams a session at most once per connection, so a client SHALL keep at most one `attach` for a session in flight on a connection, and SHALL send `detach` for a session only when none of its views shows that session or waits for it. Binary output frames for a session that arrive after the client sent `attach` and before the matching `attached` event are that session's buffered output; the client SHALL display them, in order, before any output that follows. When a view is replaced by another view for the same session while its attach is in flight, the new view SHALL take over that attach and receive its buffered output and all later output. Historical output SHALL NOT perform clipboard operations or produce replies to terminal report requests. Live output after `attached` SHALL retain its normal rendering behavior even when buffered before a view is wired. With `session-control-v1`, only the controller SHALL send replies to live terminal report requests; observers MUST NOT reply, claim control or resize to restore a view. Without that capability, existing client behavior applies.

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
- **AND** live query replies after `attached` are sent only by the controller on a control-capable server, while clipboard writes follow the clipboard policy

#### Scenario: Two views receive a terminal query
- **WHEN** a controller and observer receive the same live terminal report request
- **THEN** only the controller sends its reply, and neither view sends replies for historical queries

#### Scenario: Observer mounts or rebuilds
- **WHEN** an observing view attaches or is rebuilt from a screen snapshot
- **THEN** it sends no redraw resize and preserves the host's current dimensions


## ADDED Requirements

### Requirement: Session Control State
The server SHALL maintain at most one controlling connection per session and publish `control.state` with owner, generation and PTY size on attach and after control or size changes. Claim and release messages SHALL be correlated with their results. A claim SHALL require the last observed generation; taking an occupied session SHALL additionally require explicit takeover intent. Rejected claims SHALL return current state without modifying ownership.

#### Scenario: Claim an unowned session
- **WHEN** a connection claims an unowned session using the current generation
- **THEN** it becomes the sole controller and subscribers receive a fresh control generation

#### Scenario: Concurrent takeovers
- **WHEN** two observers request takeover using the same observed generation
- **THEN** at most one succeeds and the other receives a stale-state result without replacing the winner

#### Scenario: Viewing an occupied session
- **WHEN** another connection attaches to a controlled session without requesting takeover
- **THEN** it receives output and current control/size state without changing ownership or size

#### Scenario: Unknown session claim
- **WHEN** a claim names a nonexistent session
- **THEN** it fails with `session not found` and creates no session or control record

### Requirement: Session Control Lifetime
Control SHALL be connection-scoped, ephemeral and released on explicit owner release, owner detach, disconnect or session end. Every ownership transition SHALL invalidate previous generations. Release requests from non-owners or stale generations MUST NOT affect current ownership. A reconnect SHALL obtain a new identity and claim afresh; released sessions SHALL retain their last PTY dimensions.

#### Scenario: Detach or release control
- **WHEN** the controller detaches or explicitly releases its current control
- **THEN** the session becomes unowned with a new generation, retains its size and keeps running

#### Scenario: Delayed release after another takeover
- **WHEN** a former controller sends release with an old generation
- **THEN** the new controller and its generation remain unchanged

#### Scenario: Session ends or daemon restarts
- **WHEN** a controlled session ends or its daemon restarts
- **THEN** its old control generation cannot authorize a later input or resize, including for a restored session ID
