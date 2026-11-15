## ADDED Requirements

### Requirement: Welcome Announces Capabilities
Immediately after a completed native service negotiation, without waiting for any client message, the server SHALL send `mux.welcome` carrying its `capabilities` (shape in `docs/protocol.ts`). The welcome SHALL carry no screen list, no connection identity and no server mode: a client reads the workspace with `mux.workspace_resync` (see Ordered Workspace Catalog) and the mode with `GET /api/mux/status` (see `server-modes`).

#### Scenario: Welcome before any request
- **WHEN** a client completes the native service negotiation and sends nothing
- **THEN** it receives `mux.welcome` with the server's `capabilities`

#### Scenario: Workspace comes from the catalog
- **WHEN** a client connects to a server whose workspace holds screens `C`, `A`, `B` in that order and sends `mux.workspace_resync`
- **THEN** the welcome lists no screens, and the `mux.workspace_snapshot` lists `C`, `A`, `B` in that order

### Requirement: Pane State From Session Snapshots
Session snapshots SHALL carry `displayTitle` and `busy`, and an ordinary terminal pane's title SHALL match its session's display title. `busy` denotes a known foreground process other than the session's original shell, not agent thinking; exited sessions are not busy. `displayTitle` SHALL use the last title a program other than the original shell set with OSC 0 or OSC 2 when there is one, else the foreground process name; for known shell names it SHALL include the working directory, shortened under the server user's home. Display titles SHALL be plain text bounded to 500 characters. Process name and working directory SHALL be known on Linux and macOS; on other platforms they MAY be empty. The server SHALL refresh these values while no client is attached and SHALL close panes on exit itself (see `terminal-sessions`, Process Exit); clients SHALL use these values and leave automatic exit closure to the server.

#### Scenario: Reconnect to an unattended pane
- **WHEN** a process changes while no window shows its pane and a client later resynchronizes
- **THEN** the snapshot gives its current `displayTitle` and `busy` state and the pane title matches

#### Scenario: Foreground program changes
- **WHEN** the user starts `vim` in a pane's shell
- **THEN** within about a second the session's next snapshot gives `busy` true and `displayTitle` `vim`

#### Scenario: Program sets a window title
- **WHEN** a program other than the shell in a pane's session sets the OSC 2 title `build`
- **THEN** the session's next snapshot gives `displayTitle` `build`

### Requirement: Relative Screen Placement Announcement
A supporting service SHALL announce `screen-placement-v1` in `mux.welcome.capabilities`. A native client SHALL require that capability and `workspace-catalog-v1` before sending `move_screen`. The operation SHALL use the existing ordered session protocol, rejection reporting and workspace-change propagation.

#### Scenario: Supported relative placement
- **WHEN** a client submits a valid `move_screen` to a supporting service followed by `mux.workspace_resync`
- **THEN** the response reflects the operation against current state, the sender receives no success acknowledgement, and terminal delivery continues
- **AND** a changed order produces one workspace `mux_op` nudge without a Screen identifier to other clients

#### Scenario: Unsupported local service
- **WHEN** either required capability is absent from the assigned local service's welcome
- **THEN** native local Screen placement is unavailable and sends neither `move_screen` nor a full-list replacement fallback
- **AND** this does not disable window-local arrangement of remote references

#### Scenario: Parsed placement rejection
- **WHEN** `move_screen` carries an invalid moved Screen or insertion anchor
- **THEN** only the sender receives `mux.op_failed` naming the operation and moved Screen, and it requests authoritative catalog state without retrying the move
- **AND** other clients receive no change nudge

#### Scenario: Invalid anchor representation
- **WHEN** a recognized `move_screen` request supplies a non-string non-null insertion anchor
- **THEN** it changes no workspace state and is not treated as an omitted anchor or end placement


## MODIFIED Requirements

### Requirement: Concurrent Clients Without Screen Ownership
The server SHALL accept multiple simultaneous connections and MUST NOT grant any connection exclusive access to a screen. There are no `mux.connect`, `mux.attach`, `mux.detach`, or `mux.busy` messages; every connected client can read and modify every screen.

#### Scenario: Second client connects
- **WHEN** a second client connects while a first client is connected
- **THEN** both clients SHALL receive their own `mux.welcome` and the same workspace catalog
- **AND** ops from either client SHALL be applied to the shared workspace

### Requirement: Client Resynchronization
A client MUST treat the server's workspace state as authoritative after connecting: after `mux.welcome` it SHALL request the workspace catalog with `mux.workspace_resync`, SHALL create any screen the `mux.workspace_snapshot` lists that it does not have locally, and SHALL send `mux.resync` for every listed screen. A client SHALL build every listed screen from its `mux.screen_snapshot`, also when it has prepared the screen locally before the snapshot arrives, and MUST NOT send `create_screen` for such a screen in its place. (Re-creating a screen that later ran empty stays allowed, see `workspace-model`.) Before the first workspace snapshot of a connection a client MUST NOT send `create_screen`. It SHALL create an initial screen of its own only when the catalog lists no screens, with `onlyIfEmpty: true` (see `workspace-model`, First Screen of an Empty Workspace), and SHALL show only the listed screens when the catalog lists any. When the server refuses that initial screen, the client SHALL drop it and show the screens the server has instead. On a `mux.nudge` that carries a `screenId`, a client SHALL send `mux.resync` for that screen.

#### Scenario: Reconnect restores state
- **WHEN** a client reconnects and the workspace snapshot lists screens `s1` and `s2`
- **THEN** the client SHALL send `mux.resync` for `s1` and for `s2`

#### Scenario: Nudge triggers resync
- **WHEN** a client receives `mux.nudge` with `screenId` `s1`
- **THEN** the client SHALL send `mux.resync` for `s1`

#### Scenario: Page reload against an existing workspace
- **WHEN** a window starts while the server's workspace holds screen `s1` whose panes run sessions
- **THEN** the window SHALL send `mux.resync` for `s1` and no `create_screen`
- **AND** its view of `s1` SHALL show those panes connected to their existing sessions

#### Scenario: First start against an empty workspace
- **WHEN** a client starts and the workspace snapshot lists no screens
- **THEN** the client SHALL create one screen with `create_screen` and `onlyIfEmpty: true`

#### Scenario: Two windows start against an empty workspace
- **WHEN** two windows start together, both workspace snapshots list no screens and both send their initial `create_screen` with `onlyIfEmpty: true`
- **THEN** the server holds exactly one screen
- **AND** the window whose create was refused drops its own screen and shows the one the server holds

#### Scenario: Older server
- **WHEN** a server without `screen-names-v1` ignores `onlyIfEmpty` and two windows start together against its empty workspace
- **THEN** each window's initial screen is created, as before

### Requirement: Pane Spawn Notification
When an op submitted over a client connection successfully starts a session for a pane, the server SHALL send `mux.pane_spawned` with that `paneId` and the new `sessionId` to the originating client, and SHALL enqueue it before processing that client's next message. A failed spawn SHALL send `mux.op_failed` with the pane and actual screen identifiers instead, and SHALL never advertise the absent session as spawned. A client receiving `mux.pane_spawned` SHALL connect the pane to the given session with `attach` (see `terminal-sessions`).

#### Scenario: Split creates a session
- **WHEN** a client sends a `mux.op` that creates pane `p2` with a new session
- **THEN** the client SHALL receive `mux.pane_spawned` with `paneId` `p2` and the server-assigned `sessionId`

#### Scenario: Job program cannot start
- **WHEN** a job op is accepted but execution fails
- **THEN** the originating client receives failure rather than a successful spawn event, and synchronized job state reports `failed`

### Requirement: Classified Notification Events
The server SHALL attach `notice` to every `process.finished`, `process.idle` and `session.exited` event and to every `program_status.changed` event marked `notify`, and to no other `program_status.changed` (shapes in `docs/protocol.ts`). A notice SHALL carry `severity` (`info`, `success`, `warning` or `error`), a plain-text `title`, `setting` when a `notifications.*` switch besides `enabled` governs it, and `suppressed` when no window should notify:

- `process.finished`: `info`, `<process> finished`, setting `processFinished`.
- `process.idle`: `info`, `<process> is waiting` or `Command finished`; suppressed `program-status` while the session holds a root program status record.
- `session.exited`: with exit code 0 `info`, `<process> exited`, setting `processExited`; otherwise `error`, `<process> failed`, setting `processCrashed`, where `<process>` is `session` when none is known; suppressed `user-initiated` when `userInitiated` is true.
- `program_status.changed`: `warning` for `blocked`, `success` for `done`, `error` for `error`, setting `programStatus`; the title names the screen (its name, else `Unnamed Screen`, never a label by its place) followed by the state in words and the record's `msg`, else its `title`.

A session SHALL exit user-initiated when the server closed it while its program was still running, when its pane or screen was closed.

#### Scenario: Process finished
- **WHEN** a streamed session's foreground program `cargo` ends and the shell is in the foreground again
- **THEN** every connection receives `process.finished` with a notice of severity `info`, title `cargo finished` and setting `processFinished`

#### Scenario: Session failed
- **WHEN** a streamed session's program exits with code 1 by itself
- **THEN** its `session.exited` carries a notice of severity `error`, setting `processCrashed` and no `suppressed`

#### Scenario: Pane closed by the user
- **WHEN** a pane is closed while a connection streams its session and the session's shell still runs
- **THEN** the `session.exited` for it has `userInitiated` true and its notice is suppressed `user-initiated`

#### Scenario: Waiting program that reports its status
- **WHEN** a session holds a `working` root record and its shell integration reports that a command finished
- **THEN** its `process.idle` carries a notice suppressed `program-status`

#### Scenario: Blocked program
- **WHEN** the root record of a session on the screen named `build` changes to `blocked` with the kind `permission` and the message `Run tests?`, and the change is marked `notify`
- **THEN** the event carries a notice of severity `warning`, setting `programStatus` and title `build: Waiting for permission: Run tests?`

#### Scenario: Unmarked status change
- **WHEN** a `program_status.changed` is not marked `notify`
- **THEN** it carries no notice

#### Scenario: Status of an unnamed screen
- **WHEN** a marked `blocked` change comes from a session on a screen without a name, second in the catalog
- **THEN** its notice's title starts with `Unnamed Screen: `, not with `Screen 2: `

### Requirement: Notifications in the Window
A window SHALL add a notification for an event with a notice that is not suppressed, while `notifications.enabled` and the switch the notice names are on in the window's settings, and SHALL take its severity and title from the notice; a notification of a host's event SHALL be prefixed with the host's name. It SHALL add nothing for a suppressed notice. A notification for the pane the user watches (the focused pane of the shown screen, in a focused window) SHALL be recorded as read and show no notice in the window or from the system. Turning a switch off SHALL also remove the notifications the window holds that it governs, and turning it on again SHALL NOT bring them back.

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
- **THEN** the notification goes and does not return when the switch is turned on again

## REMOVED Requirements

### Requirement: Welcome on Connect
**Reason**: The welcome's `clientId`, `screens` and `serverMode` were read only by the web client; the welcome keeps its capabilities (see Welcome Announces Capabilities).
**Migration**: Read the workspace with `mux.workspace_resync` and the mode with `GET /api/mux/status`.

### Requirement: Per-Connection Client Identity
**Reason**: `clientId` is no longer sent; no remaining client read it. Per-connection identities stay inside the server.
**Migration**: None.

### Requirement: Server Derived Pane State
**Reason**: The capability `serverDerivedPaneState` was read only by the web client; every supported server derives pane state itself. The behavior moves to Pane State From Session Snapshots.
**Migration**: None.

### Requirement: Relative Screen Placement Capability
**Reason**: Its compatibility clause for `reorder_screens` goes with that operation; the rest moves to Relative Screen Placement Announcement.
**Migration**: Send `move_screen`.
