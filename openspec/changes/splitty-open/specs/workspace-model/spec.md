## MODIFIED Requirements

### Requirement: Workspace Structure
The server SHALL hold exactly one workspace consisting of a set of screens, an explicit screen order, and at most one active screen. Each screen SHALL carry an ID, an optional name, a revision counter, a root layout tree, a list of floating entries, a pane table, and a session table in which every terminal pane created by a layout operation references exactly one session. A viewer pane references no session (see `viewer-panes`). The workspace SHALL additionally hold managed-job records and deduplication tombstones. A job pane/session SHALL identify its job, including when its process is no longer running; removing a pane SHALL not silently reuse or erase the job's identity.

#### Scenario: Fresh server without prior state
- **WHEN** the server starts with no persisted workspace
- **THEN** the workspace contains no screens, an empty screen order, no active screen and no managed jobs

#### Scenario: Screen listings follow the screen order
- **WHEN** a client or the CLI lists the screens of the workspace
- **THEN** the screens are returned in the workspace's screen order

#### Scenario: A viewer pane next to a terminal
- **WHEN** a screen holds a terminal pane and a viewer pane
- **THEN** its session table holds exactly one session, the terminal pane's

### Requirement: Layout Operations
The server SHALL change the workspace only through operations: workspace-level `create_screen`, `close_screen`, `rename_screen`, `set_active_screen`, `reorder_screens`, `move_pane_to_screen`, and screen-level `split_pane`, `new_tab`, `close_pane`, `resize_split`, `move_pane`, `arrange_panes`, `select_tab`, `float_pane`, `dock_pane`, `move_float`, `set_scratchpad`, `open_viewer`, plus `create_job`, `stop_job`, `forget_job`, and trusted job lifecycle/ownership operations. An operation SHALL have the same effect on the model whether it arrives over `mux-protocol` or `control-api`, except `set_scratchpad`, which only `mux-protocol` accepts (see `session-scratchpad`). Job operations SHALL preserve the authorization and idempotency rules of `managed-jobs` at every entry point; lifecycle and ownership updates SHALL not be accepted as arbitrary client-authored state.

#### Scenario: Same op from two transports
- **WHEN** an identical `split_pane` op is applied once via a WebSocket client and once via the HTTP control API against identical workspaces
- **THEN** both resulting workspaces have the same layout structure and both spawn one new session

#### Scenario: Screen-level op on an unknown screen
- **WHEN** a screen-level op names a screen ID that does not exist
- **THEN** the server rejects the op with a "screen not found" error

#### Scenario: Job start through two interfaces
- **WHEN** the same authorized `create_job` intent and request ID reach the job API and mux API
- **THEN** both resolve to the same job and layout without a second session

#### Scenario: Scratchpad only through the window
- **WHEN** a `set_scratchpad` op arrives over the control API
- **THEN** the server refuses it, and the workspace stays unchanged

#### Scenario: Viewer from both transports
- **WHEN** an identical `open_viewer` op is applied once via a WebSocket client and once via the HTTP control API against identical workspaces
- **THEN** both resulting workspaces hold the same viewer pane in the same place, and neither spawns a session

### Requirement: Split Session Inheritance
The session of a pane created by `split_pane` or `new_tab` SHALL start in the op's `cwd` if given, else in the source pane's known working directory, else in the source session's working directory. It SHALL use the op's shell profile if given, else the source session's shell profile, else, for a source pane without a session such as a viewer pane, the default shell profile. The source session's legacy shell value SHALL be inherited only when the op names no shell profile.

#### Scenario: Inherit profile and directory
- **WHEN** a pane whose session uses profile `zsh` in `/work` is split without `cwd` or profile
- **THEN** the new session uses profile `zsh` and starts in `/work`

#### Scenario: Explicit profile overrides inheritance
- **WHEN** a pane whose session has a legacy shell value is split with an explicit shell profile
- **THEN** the new session uses the explicit profile and carries no legacy shell value

#### Scenario: Split of a viewer pane
- **WHEN** a viewer pane of `/home/me/app/PLAN.md` is split without `cwd` or profile
- **THEN** the new terminal's session starts in `/home/me/app` with the default shell profile

### Requirement: Closing a Pane
On `close_pane` the server SHALL require that the pane exists in the root layout or in a floating entry, SHALL end the pane's session when it has one, and SHALL remove the pane from the model. If the pane's slot holds other panes, only the pane SHALL be removed from the stack. Otherwise the slot SHALL be removed from its parent split, whose sizes become `resultingSizes` or equal fractions, and a split left with a single child SHALL be replaced by that child. Closing the last pane of the root layout SHALL leave the screen with an empty root slot. Closing the last pane of a floating entry SHALL remove the floating entry.

#### Scenario: Close one side of a split
- **WHEN** `close_pane` is applied to one pane of a two-slot split
- **THEN** the split is replaced by the remaining slot
- **AND** the closed pane's session is ended

#### Scenario: Close the last pane of a screen
- **WHEN** `close_pane` is applied to the only pane of a screen
- **THEN** the screen still exists with an empty root slot and no panes

#### Scenario: Close an unknown pane
- **WHEN** `close_pane` names a pane the screen does not contain
- **THEN** the op is rejected and no session is ended

#### Scenario: Close a viewer pane
- **WHEN** `close_pane` is applied to a viewer pane
- **THEN** the pane is removed and no session is ended

### Requirement: Identifier Assignment
The server SHALL use a client-supplied `screenId` (on `create_screen`) and `newPaneId` (on `create_screen`, `split_pane`, `new_tab` and `open_viewer`) verbatim as the new screen's or pane's ID. A `newPaneId` that already names a pane anywhere in the workspace, in a layout or a floating entry, SHALL make the op invalid, and it SHALL be rejected atomically (see Atomic Rejection). When those IDs are omitted the server SHALL generate short prefixed IDs (`screen-…`, `pane-…`). Session, layout node and floating entry IDs SHALL always be generated by the server.

#### Scenario: Client-supplied pane ID
- **WHEN** a client sends `split_pane` with `newPaneId` `terminal-5`
- **THEN** the new pane in the model has ID `terminal-5`
- **AND** its session has a server-generated ID of the form `sess-<8 hex chars>`

#### Scenario: Server-generated screen ID
- **WHEN** `create_screen` is applied without a `screenId`
- **THEN** the new screen gets an ID of the form `screen-<8 hex chars>`

#### Scenario: Pane ID already in use
- **WHEN** a client sends `split_pane` with `newPaneId` `terminal-5` while a pane `terminal-5` exists on another screen
- **THEN** the op is rejected, the workspace is unchanged, and no session is spawned

#### Scenario: Client-supplied viewer pane ID
- **WHEN** a client sends `open_viewer` with `newPaneId` `viewer-0123456789abcdef`
- **THEN** the new viewer pane has that ID, and no session is created
