## MODIFIED Requirements

### Requirement: Workspace Persistence In Daemon Mode
A daemon SHALL persist the workspace to `state.json` in the Splitty configuration directory after every successfully applied workspace change, replacing the file atomically so that a reader never sees a partially written file. A rejected change SHALL NOT trigger a write. A failed write SHALL be logged and SHALL NOT stop the daemon. The persisted workspace SHALL include the screens with their names, order, and active screen, each screen's layout including floating windows, the panes, for each session the record needed to start it again (command, working directory, shell or shell profile, status) and its scratchpad (see `session-scratchpad`), and for each viewer pane its `viewer` record with the text of a `text` document (see `viewer-panes`) and its annotations (see `viewer-annotations`). It SHALL additionally include managed-job identity, launch metadata, owner/creator identifiers, lifecycle and deduplication records, with an explicit never-auto-respawn marker on job sessions. It SHALL NOT include terminal output or scrollback, the content of files that viewer panes show, running process state, client connections, grant socket paths or live grant authority. File permissions are governed by `access-control`.

#### Scenario: Layout change persisted
- **WHEN** a client splits a pane on a daemon
- **THEN** `state.json` contains the new pane and its session record once the change has been applied

#### Scenario: Rejected change
- **WHEN** a workspace operation is rejected as invalid
- **THEN** `state.json` is not rewritten

#### Scenario: Abrupt termination
- **WHEN** the daemon process is killed without a chance to shut down
- **THEN** `state.json` still holds the workspace as of the last successfully applied change

#### Scenario: Job lifecycle persisted without a reader
- **WHEN** a managed job exits without an attached UI
- **THEN** its terminal result is persisted without waiting for a layout change, and no output or socket authority is written

#### Scenario: Scratchpad persisted
- **WHEN** a client saves text in a session's scratchpad on a daemon
- **THEN** `state.json` holds that text in the session's record once the change has been applied

#### Scenario: Viewer panes persisted
- **WHEN** a client opens a viewer pane for a file and one for a terminal selection on a daemon
- **THEN** `state.json` holds both panes with their `viewer` records and the selection's text, and none of the file's content

#### Scenario: Annotations persisted
- **WHEN** a client adds an annotation to a viewer pane on a daemon
- **THEN** `state.json` holds the annotation in the pane's `viewer` record once the change has been applied
