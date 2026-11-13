# Spec Delta

## ADDED Requirements

### Requirement: Notification Notices Capability
A server that classifies its notification events SHALL list `notification-notices-v1` in `mux.welcome.capabilities`. For a connection without it, a window SHALL add for each `process.finished`, `process.idle`, `session.exited` and `notification` event an `info` notification titled `Activity in <process>`, or `Session activity` when the event names no process, while `notifications.enabled` is on, and SHALL derive nothing else from the event; a `program_status.changed` event without `notice` SHALL add no notification.

#### Scenario: Current server
- **WHEN** a client connects to a server of this version
- **THEN** `mux.welcome.capabilities` lists `notification-notices-v1`

#### Scenario: Older server
- **WHEN** a window is connected to a host whose `mux.welcome` does not list `notification-notices-v1` and the host sends `process.finished` for `cargo`, which the user is not watching
- **THEN** the window adds an `info` notification titled `Activity in cargo`, prefixed with the host's name

### Requirement: Classified Notification Events
The server SHALL attach `notice` to every `process.finished`, `process.idle` and `session.exited` event and to every `program_status.changed` event marked `notify`, and to no other `program_status.changed` (shapes in `docs/protocol.ts`). A notice SHALL carry `kind`, `severity` (`info`, `success`, `warning` or `error`), a plain-text `title`, `setting` when a `notifications.*` switch besides `enabled` governs it, and `suppressed` when no window should notify:

- `process.finished`: kind `process.finished`, `info`, `<process> finished`, setting `processFinished`.
- `process.idle`: kind `process.idle`, `info`, `<process> is waiting` or `Command finished`; suppressed `program-status` while the session holds a root program status record.
- `session.exited`: with exit code 0 kind `session.exited`, `info`, `<process> exited`, setting `processExited`; otherwise kind `session.crashed`, `error`, `<process> failed`, setting `processCrashed`, where `<process>` is `session` when none is known; suppressed `user-initiated` when `userInitiated` is true.
- `program_status.changed`: kind `program-status`, `warning` for `blocked`, `success` for `done`, `error` for `error`, setting `programStatus`; the title names the screen (its name, else `Screen <n>` by its place in the catalog) followed by the state in words and the record's `msg`, else its `title`.

A session SHALL exit user-initiated when the server closed it while its program was still running: on a connection's `close` or when its pane or screen was closed.

#### Scenario: Process finished
- **WHEN** a streamed session's foreground program `cargo` ends and the shell is in the foreground again
- **THEN** every connection receives `process.finished` with a notice of kind `process.finished`, severity `info`, title `cargo finished` and setting `processFinished`

#### Scenario: Session failed
- **WHEN** a streamed session's program exits with code 1 by itself
- **THEN** its `session.exited` carries a notice of kind `session.crashed`, severity `error`, setting `processCrashed` and no `suppressed`

#### Scenario: Pane closed by the user
- **WHEN** a pane is closed while a connection streams its session and the session's shell still runs
- **THEN** the `session.exited` for it has `userInitiated` true and its notice is suppressed `user-initiated`

#### Scenario: Waiting program that reports its status
- **WHEN** a session holds a `working` root record and its shell integration reports that a command finished
- **THEN** its `process.idle` carries a notice suppressed `program-status`

#### Scenario: Blocked program
- **WHEN** the root record of a session on the screen named `build` changes to `blocked` with the kind `permission` and the message `Run tests?`, and the change is marked `notify`
- **THEN** the event carries a notice of kind `program-status`, severity `warning`, setting `programStatus` and title `build: Waiting for permission: Run tests?`

#### Scenario: Unmarked status change
- **WHEN** a `program_status.changed` is not marked `notify`
- **THEN** it carries no notice

### Requirement: Notifications in the Window
A window SHALL add a notification for an event with a notice that is not suppressed, while `notifications.enabled` and the switch the notice names are on in the window's settings, and SHALL take its kind, severity and title from the notice; a notification of a host's event SHALL be prefixed with the host's name. It SHALL add nothing for a suppressed notice. A notification for the pane the user watches (the focused pane of the shown screen, in a focused window) SHALL be recorded as read and show no notice in the window or from the system. The switches SHALL govern arriving events only; notifications already in the window SHALL stay until they are read or dismissed.

#### Scenario: Own pane closed
- **WHEN** the user closes a pane whose shell still runs
- **THEN** the window adds no notification and shows no notice in the window or from the system

#### Scenario: Watched pane
- **WHEN** a notice that is not suppressed arrives for the pane the user watches
- **THEN** the window records it as read and shows no notice for it

#### Scenario: Switch off
- **WHEN** `notifications.processFinished` is off and a `process.finished` arrives for a pane the user is not watching
- **THEN** the window adds no notification

#### Scenario: Switch turned off later
- **WHEN** a window holds an unread `process.finished` notification and the user turns `notifications.processFinished` off
- **THEN** the notification stays until the user reads or dismisses it
