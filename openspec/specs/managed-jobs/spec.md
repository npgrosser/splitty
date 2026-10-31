# managed-jobs Specification

## Purpose

Managed jobs let users and coding agents run independently lived processes in visible terminal panes, inspect bounded output, and recover control after the initiating client exits. Ordinary interactive terminal sessions remain distinct from managed jobs.

## Requirements

### Requirement: Stable Job Identity
An accepted job SHALL have a server-assigned `jobId`, a pane and session identity, creation time, immutable launch description and creator, and a current owner. These identities SHALL survive client disconnects and pane moves. Names and working directories SHALL be labels, never ownership proofs. A job ID SHALL never be reused for a later process.

#### Scenario: Backend outlives the agent
- **WHEN** a client launches a backend, exits, and a fresh client with the same authorization lists jobs
- **THEN** it finds the same job and running session with the same identifiers and can read subsequent output

#### Scenario: Two similarly named jobs
- **WHEN** two authorized agents launch jobs named `backend` in the same directory
- **THEN** the jobs have different identifiers and neither agent gains access to the other's job

### Requirement: Launch Validation
A launch SHALL name a nonempty program-and-arguments array and an existing absolute working directory on the server. Invalid arguments, a missing directory, a stale placement target or exceeded capacity SHALL fail before a process starts. An accepted launch SHALL create exactly one job terminal, without borrowing an existing terminal's input stream.

#### Scenario: Invalid directory
- **WHEN** the requested working directory does not exist
- **THEN** the launch fails without a pane, job or process, rather than running in the home directory

#### Scenario: Backend in a background tab
- **WHEN** a valid launch requests a tab next to the granted anchor
- **THEN** one new job pane runs the program and the existing terminal receives no input
- **AND** the user's selected tab, screen and keyboard focus are preserved

### Requirement: Truthful Launch Outcome
Launch success SHALL mean the program was spawned, not that it became ready or remains running. Spawn failure SHALL produce a failed job with an actionable error and no running session. After acceptance, the returned identifiers SHALL identify the same attempt on success, failure, or retry. A job that exits immediately SHALL retain its real result.

#### Scenario: Missing executable
- **WHEN** an accepted launch cannot execute its program
- **THEN** the result is a spawn error with the failed job's identifiers, no successful spawn event is emitted, and the pane never starts a fallback shell

#### Scenario: Immediate failure after spawn
- **WHEN** the program starts successfully and immediately exits with code 7
- **THEN** the job reports `exited` with `exitCode` 7 rather than a spawn failure or a fabricated running state

### Requirement: Idempotent Launch Requests
Launches SHALL carry a `requestId`. Within one owner and server incarnation, simultaneous or repeated requests with the same ID and normalized launch SHALL resolve to the same job attempt. Reusing it with different launch data SHALL conflict. Accepted work SHALL survive HTTP cancellation. A forgotten attempt SHALL remain a tombstone for deduplication until that incarnation ends.

#### Scenario: Response lost after spawn
- **WHEN** a launch response is lost and the caller retries with the same request ID and launch data
- **THEN** the original identifiers and outcome are returned and no second process starts

#### Scenario: Concurrent duplicate and conflicting requests
- **WHEN** concurrent requests use one request ID, two with identical data and one with a different command
- **THEN** the identical requests share one attempt and the conflicting request fails without effects

#### Scenario: Retry a forgotten attempt
- **WHEN** a client reuses the request ID of a forgotten job
- **THEN** the server reports that the attempt is gone and starts nothing

### Requirement: Job Lifecycle
Jobs SHALL report `starting`, `running`, `stopping`, `exited`, `failed` or `interrupted`. A process result SHALL distinguish an actual exit code, signal termination when available, and an unknown result. Lifecycle tracking SHALL run without UI subscribers. Ownership transfer or a pane move SHALL NOT restart a job or alter its result.

#### Scenario: Unwatched process exits
- **WHEN** a job exits while no UI or CLI reads it
- **THEN** a later inspection reports its exit result and its retained output remains readable

### Requirement: Independent Output Cursors
Job output SHALL be read non-destructively with versioned cursors bound to the job's output generation and representation. Each response SHALL identify the next unread position and whether more data is currently available. Cursor reads SHALL neither subscribe a UI, resize the terminal nor change any other reader's position. Malformed, foreign or future cursors SHALL be rejected.

#### Scenario: UI and two agents read together
- **WHEN** a UI streams a job and two authorized readers fetch output using independent cursors
- **THEN** each receives its own complete sequence of retained output without stealing, duplicating or resizing the UI stream

#### Scenario: Wrong cursor
- **WHEN** a cursor belongs to another job, representation or server incarnation, or lies beyond the output produced
- **THEN** the read fails with `invalid_cursor` and never silently restarts from a different position

### Requirement: Bounded Log Reads
Logs SHALL support an initial last-N-lines text read or a cursor read, with a default 64 KiB and maximum 256 KiB returned-data limit. Cursor and tail selection SHALL be mutually exclusive. Overwritten data SHALL be reported using `lostBytes` and the oldest available cursor. Output limits SHALL advance only over the data actually returned.

#### Scenario: Busy backend exceeds retention
- **WHEN** output overwrites bytes after a reader's last cursor
- **THEN** the next read reports the lost byte count and starts at the oldest available position

#### Scenario: Pagination with concurrent writes
- **WHEN** a response reaches its byte limit while more output is available and the process keeps writing
- **THEN** `hasMore` is true and the next cursor resumes after the returned data, with any later overflow reported explicitly

### Requirement: Text And Raw Log Representations
Logs SHALL offer raw PTY bytes as base64 and a separate incremental UTF-8 text representation that removes terminal escape/control sequences except newlines and tabs. Text parsing SHALL handle split escape sequences and UTF-8 characters across output chunks. Text is a log transcript, not a reconstructed TUI screen. Raw and text cursors SHALL not be interchangeable.

#### Scenario: Split terminal controls
- **WHEN** a job emits a color sequence, an OSC clipboard payload and a multibyte character across several writes
- **THEN** raw reads preserve those bytes and text reads retain the printable character without the control sequences or clipboard payload

### Requirement: Bounded Waiting And Output Completion
A log read SHALL wait only when no requested output is available and output is not complete, for at most the requested `waitMs` capped at 30000. Output, completion or revoked access SHALL wake it. `outputComplete` SHALL describe capture ending, separately from process exit; no final bytes SHALL be discarded merely because the child exited. Disconnecting a reader SHALL release its wait resources.

#### Scenario: Last output after exit observation
- **WHEN** process exit is observed before its last bytes have been captured
- **THEN** reads can report an exited job with `outputComplete` false and later return the remaining bytes before reporting completion

#### Scenario: Quiet process and cancelled reader
- **WHEN** a reader waits on a quiet job and its deadline expires or its connection closes
- **THEN** the deadline returns an empty bounded response, or cancellation releases the reader, without affecting the job

### Requirement: Stop And Retention
Stopping SHALL terminate the managed process as specified in `terminal-sessions`, leave its pane and buffered output inspectable, and be idempotent. It SHALL not close unrelated panes or signal processes by a name supplied by the caller. Forgetting SHALL be permitted only after a job has reached a terminal state and capture ended, and SHALL release its pane/output while retaining its deduplication tombstone.

#### Scenario: Stop twice then inspect
- **WHEN** a caller stops a running job twice
- **THEN** both requests address the same stop and subsequent inspection and log reads still show its final result and retained output

#### Scenario: Forget a running job
- **WHEN** a caller tries to forget a running or still-draining job
- **THEN** the request conflicts and neither the process nor its output is removed

### Requirement: Bounded Admission
The server SHALL admit at most 16 active jobs per agent grant, 64 globally, 4096 records including tombstones per incarnation, 64 live grants, and 128 waiting log requests with at most 8 per grant. Starting/running/stopping jobs and exited jobs with incomplete capture count as active. Admission SHALL be atomic; exceeding a limit SHALL return `capacity_exceeded` without spawning or silently evicting a live job or deduplication record.

#### Scenario: Concurrent admission at the limit
- **WHEN** two launches compete for the last job slot or two readers compete for the last wait slot
- **THEN** at most one is admitted and the other receives `capacity_exceeded` without allocated work

### Requirement: Explicit Job Ownership Transfer
Only trusted administration SHALL change a job's current owner or reclaim it for the user. Transfer SHALL preserve its creator, process and output, revoke the previous grant's access before acknowledgement, and grant access only to the selected new owner. Revoking a grant SHALL leave its jobs running and available to trusted administration.

#### Scenario: Resume after a Claude restart
- **WHEN** Claude is launched again under the same live grant
- **THEN** it can list and control that grant's existing jobs without restarting them

#### Scenario: User takes over a job
- **WHEN** trusted administration reclaims an agent-owned terminal for manual use
- **THEN** the process keeps running but the old agent can no longer read, write or stop it, including through an already waiting request
