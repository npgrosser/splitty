# workspace-model Specification

## Purpose
Defines the multiplexer domain model the server is the source of truth for: the workspace with its screens, each screen's layout tree of splits, slots and panes, floating panes, and the layout operations that change them. Transport of these operations is covered by `mux-protocol` and `control-api`; the terminal processes behind panes are covered by `terminal-sessions`.

## Requirements

### Requirement: Workspace Structure
The server SHALL hold exactly one workspace consisting of a set of screens, an explicit screen order, and at most one active screen. Each screen SHALL carry an ID, an optional name, a revision counter, a root layout tree, a list of floating entries, a pane table, and a session table in which every pane created by a layout operation references exactly one session. The workspace SHALL additionally hold managed-job records and deduplication tombstones. A job pane/session SHALL identify its job, including when its process is no longer running; removing a pane SHALL not silently reuse or erase the job's identity.

#### Scenario: Fresh server without prior state
- **WHEN** the server starts with no persisted workspace
- **THEN** the workspace contains no screens, an empty screen order, no active screen and no managed jobs

#### Scenario: Screen listings follow the screen order
- **WHEN** a client or the CLI lists the screens of the workspace
- **THEN** the screens are returned in the workspace's screen order

### Requirement: Layout Tree Shape
A screen's layout tree SHALL consist of two node kinds: a `slot`, which holds an ordered stack of panes (shown as tabs) and an optional active pane, and a `split`, which holds an axis, two or more ordered children, and one size per child. A `horizontal` split SHALL lay out its children left to right and a `vertical` split SHALL lay them out top to bottom. Sizes SHALL be fractions of the parent's extent.

#### Scenario: Horizontal split of two panes
- **WHEN** a screen's root is a `horizontal` split with children `[slot A, slot B]` and sizes `[0.5, 0.5]`
- **THEN** slot A occupies the left half and slot B the right half of the screen

#### Scenario: Stacked panes share one slot
- **WHEN** a slot holds panes `[p1, p2]`
- **THEN** both panes occupy the same area and only one of them is shown at a time

### Requirement: Layout Operations
The server SHALL change the workspace only through operations: workspace-level `create_screen`, `close_screen`, `rename_screen`, `set_active_screen`, `reorder_screens`, `move_pane_to_screen`, and screen-level `split_pane`, `new_tab`, `close_pane`, `resize_split`, `move_pane`, `arrange_panes`, `select_tab`, `float_pane`, `dock_pane`, `move_float`, `set_scratchpad`, plus `create_job`, `stop_job`, `forget_job`, and trusted job lifecycle/ownership operations. An operation SHALL have the same effect on the model whether it arrives over `mux-protocol` or `control-api`, except `set_scratchpad`, which only `mux-protocol` accepts (see `session-scratchpad`). Job operations SHALL preserve the authorization and idempotency rules of `managed-jobs` at every entry point; lifecycle and ownership updates SHALL not be accepted as arbitrary client-authored state.

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

### Requirement: Atomic Rejection
The server MUST validate every operation against the current state and MUST leave the whole workspace unchanged when an operation is rejected. A rejected operation SHALL NOT spawn or end any session.

#### Scenario: Split of an unknown pane
- **WHEN** a client sends `split_pane` for a pane ID the screen does not contain
- **THEN** the op is rejected with a "pane not found" error
- **AND** all screens, panes and sessions remain exactly as before

#### Scenario: Later ops still apply after a rejection
- **WHEN** an op is rejected and a valid op for the same screen follows
- **THEN** the valid op is applied against the unchanged state

### Requirement: Screen Revision
The server SHALL increment a screen's revision by exactly one for every successful screen-level operation and for every successful `rename_screen` on it. A screen created or re-created by `create_screen` SHALL have revision 1.

#### Scenario: Revision after a split
- **WHEN** a successful `split_pane` is applied to a screen with revision 4
- **THEN** the screen's revision is 5

#### Scenario: Rejected op keeps the revision
- **WHEN** a screen-level op on a screen with revision 4 is rejected
- **THEN** the screen's revision stays 4

### Requirement: Identifier Assignment
The server SHALL use a client-supplied `screenId` (on `create_screen`) and `newPaneId` (on `create_screen`, `split_pane` and `new_tab`) verbatim as the new screen's or pane's ID. A `newPaneId` that already names a pane anywhere in the workspace, in a layout or a floating entry, SHALL make the op invalid, and it SHALL be rejected atomically (see Atomic Rejection). When those IDs are omitted the server SHALL generate short prefixed IDs (`screen-…`, `pane-…`). Session, layout node and floating entry IDs SHALL always be generated by the server.

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

### Requirement: Screen Creation
On `create_screen` the server SHALL create a screen whose root layout is a single slot holding one new `terminal` pane backed by one new session. The session SHALL use the shell profile given in the op, or the server's default shell profile when none is given. A new screen SHALL be appended to the end of the screen order, and SHALL become the active screen only if the workspace had no active screen. If `screenId` names an existing screen without any pane, the server SHALL re-create that screen in place: its layout becomes a single slot with the new pane, it keeps its name unless the op gives one, and it keeps its place in the screen order. If `screenId` names an existing screen that has at least one pane, in its root layout or in a floating entry, the server SHALL reject the op with a "screen already exists" error.

#### Scenario: First screen becomes active
- **WHEN** `create_screen` is applied to a workspace without screens
- **THEN** the workspace contains one screen with one pane and one session
- **AND** that screen is the active screen

#### Scenario: Additional screen does not steal activation
- **WHEN** `create_screen` is applied while another screen is active
- **THEN** the new screen is appended to the screen order
- **AND** the active screen is unchanged

#### Scenario: Re-create a screen that ran empty
- **WHEN** `create_screen` names screen `s1`, whose last pane was closed, with `newPaneId` `p9`
- **THEN** `s1` holds the single pane `p9` with a new session
- **AND** `s1` keeps its name and its place in the screen order

#### Scenario: Existing screen with panes is not replaced
- **WHEN** `create_screen` names screen `s1`, which has two panes with running sessions
- **THEN** the op is rejected with a "screen already exists" error
- **AND** `s1`, its panes and its sessions remain exactly as before

### Requirement: Screen Closing
On `close_screen` the server SHALL remove the screen, end every session belonging to it, and remove it from the screen order. If the closed screen was active, the first screen remaining in the order SHALL become active, or no screen when none remains.

#### Scenario: Closing the active screen
- **WHEN** the active screen is closed while screens B and C remain in that order
- **THEN** screen B becomes the active screen
- **AND** every session of the closed screen is ended

#### Scenario: Closing an unknown screen
- **WHEN** `close_screen` names a screen that does not exist
- **THEN** the op is rejected and no session is ended

### Requirement: Screen Rename, Activation and Order
The server SHALL apply `rename_screen` and `set_active_screen` only to existing screens and SHALL reject them otherwise. On `reorder_screens` the server SHALL set the screen order to the given list of screen IDs, ignoring IDs that do not name an existing screen.

#### Scenario: Activate an unknown screen
- **WHEN** `set_active_screen` names a screen that does not exist
- **THEN** the op is rejected and the active screen is unchanged

#### Scenario: Reorder with a stale ID
- **WHEN** `reorder_screens` is applied with `[C, X, A, B]` where X does not exist
- **THEN** the screen order becomes `[C, A, B]`

### Requirement: Splitting a Pane
On `split_pane` the server SHALL require that the source pane exists and lies in the screen's root layout; panes in floating entries cannot be split. The server SHALL add a new `terminal` pane with a new session in a new slot next to the source pane's slot, after it by default or before it when `newPaneIndex` is 0. If the source slot's parent split has the requested axis, the new slot SHALL be inserted as a sibling and the parent's sizes SHALL be set to `resultingSizes` or to equal fractions. Otherwise the source slot SHALL be replaced by a new split of the requested axis containing both slots, with sizes `resultingSizes` or `[0.5, 0.5]`. When given, `resultingSizes` MUST consist of non-negative values summing to 1 within 0.01.

#### Scenario: Split a lone pane
- **WHEN** `split_pane` with axis `horizontal` is applied to the only pane `p1` of a screen
- **THEN** the root becomes a `horizontal` split with children `[slot(p1), slot(new pane)]` and sizes `[0.5, 0.5]`
- **AND** one new session is spawned for the new pane

#### Scenario: Split along the parent's axis
- **WHEN** `split_pane` with axis `horizontal` is applied to a pane whose slot is a child of a `horizontal` split with two children
- **THEN** the parent split gains a third child directly after the source slot
- **AND** the parent's sizes become three equal fractions when no `resultingSizes` is given

#### Scenario: Invalid resulting sizes
- **WHEN** `split_pane` carries `resultingSizes` `[0.5, 0.3]`
- **THEN** the op is rejected and the layout is unchanged

### Requirement: Creating a Tab
On `new_tab` the server SHALL require that `paneId` exists in a slot of the screen's root layout or a floating entry. It SHALL append a new terminal pane to that same slot, mark it as the slot's active pane and spawn exactly one session. Existing split nodes, slot IDs, sizes and floating bounds SHALL remain unchanged. Frontend new-tab actions SHALL send `new_tab`; snapshots SHALL restore panes of one slot as tabs in one group, including floating groups.

#### Scenario: New tab shared by two clients
- **WHEN** client A creates a tab beside `p1` while client B has the same screen open
- **THEN** the server's slot contains `[p1, new pane]`
- **AND** client B's resync restores both panes in one tab group without adding a split
- **AND** a client reconnecting later restores the same tab grouping and order

#### Scenario: New tab in an existing split or floating group
- **WHEN** `new_tab` targets a pane in one group of a split screen or in a floating group
- **THEN** only that group's tab stack gains the new pane
- **AND** all other groups and their sizes remain unchanged

### Requirement: Split Session Inheritance
The session of a pane created by `split_pane` or `new_tab` SHALL start in the op's `cwd` if given, else in the source pane's known working directory, else in the source session's working directory. It SHALL use the op's shell profile if given, else the source session's shell profile. The source session's legacy shell value SHALL be inherited only when the op names no shell profile.

#### Scenario: Inherit profile and directory
- **WHEN** a pane whose session uses profile `zsh` in `/work` is split without `cwd` or profile
- **THEN** the new session uses profile `zsh` and starts in `/work`

#### Scenario: Explicit profile overrides inheritance
- **WHEN** a pane whose session has a legacy shell value is split with an explicit shell profile
- **THEN** the new session uses the explicit profile and carries no legacy shell value

### Requirement: Closing a Pane
On `close_pane` the server SHALL require that the pane exists in the root layout or in a floating entry, SHALL end the pane's session, and SHALL remove the pane from the model. If the pane's slot holds other panes, only the pane SHALL be removed from the stack. Otherwise the slot SHALL be removed from its parent split, whose sizes become `resultingSizes` or equal fractions, and a split left with a single child SHALL be replaced by that child. Closing the last pane of the root layout SHALL leave the screen with an empty root slot. Closing the last pane of a floating entry SHALL remove the floating entry.

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

### Requirement: Resizing a Split
On `resize_split` the server SHALL require that the node exists in the screen's root layout and is a split, and that `sizes` has exactly one non-negative entry per child summing to 1 within 0.01; it SHALL then replace the split's sizes.

#### Scenario: Valid resize
- **WHEN** `resize_split` sets a two-child split to `[0.3, 0.7]`
- **THEN** the split's sizes are `[0.3, 0.7]`

#### Scenario: Resize targets a slot
- **WHEN** `resize_split` names a slot node
- **THEN** the op is rejected with a "not a split" error

#### Scenario: Wrong number of sizes
- **WHEN** `resize_split` sends two sizes for a three-child split
- **THEN** the op is rejected and the sizes are unchanged

### Requirement: Moving a Pane
On `move_pane` the server SHALL require an existing pane in the root layout and either a `targetSlotId` naming a slot of the root layout or a `direction` together with a `targetSplitId`, rejecting the op otherwise. The pane SHALL be removed from its source slot; a source slot left empty SHALL be removed from its parent split (sizes become `sourceResultingSizes` or equal fractions, and a single-child split collapses). With `targetSlotId` the pane SHALL be appended to that slot's stack and become its active pane. With `direction` (`left`, `right`, `above`, `below`) the target node named by `targetSplitId`, a node ID or a pane ID standing for that pane's slot, SHALL be wrapped in a new split whose axis follows the direction, with the pane's new slot placed on the given side and sizes `resultingSizes` or `[0.5, 0.5]`. When the target no longer exists once the pane is removed (the pane's own slot it left empty, or a split that collapsed), the server SHALL reject the op. A rejected move SHALL leave the layout unchanged. Moving a pane SHALL NOT affect its session.

#### Scenario: Move into another tab stack
- **WHEN** pane `p2` is moved with `targetSlotId` naming the slot that holds `p1`
- **THEN** that slot holds `[p1, p2]` with `p2` active
- **AND** `p2`'s former slot is removed if it became empty

#### Scenario: Move to the left of a pane
- **WHEN** pane `p3` is moved with `direction` `left` and `targetSplitId` naming pane `p1`
- **THEN** `p1`'s slot is replaced by a `horizontal` split with children `[slot(p3), slot(p1)]`

#### Scenario: Move without a target
- **WHEN** `move_pane` carries neither `targetSlotId` nor `direction`
- **THEN** the op is rejected and the layout is unchanged

#### Scenario: Move to an unknown slot
- **WHEN** `move_pane` names a `targetSlotId` that is no slot of the root layout
- **THEN** the op is rejected and the pane stays where it was

#### Scenario: Direction without a target node
- **WHEN** `move_pane` carries a `direction` but no `targetSplitId`
- **THEN** the op is rejected and the layout is unchanged

#### Scenario: The move removes its own target
- **WHEN** the root is a `horizontal` split `[slot(p1), slot(p2)]` and `p1` is moved with `direction` `left` and `targetSplitId` naming that split
- **THEN** the op is rejected, and the root is still that split with both panes

### Requirement: Floating Panes
On `float_pane` the server SHALL require a pane in the root layout and SHALL reject the op when that pane is the last pane of the root layout. The pane SHALL be removed from the root layout (a parent split's sizes become equal fractions and a single-child split collapses) and placed in a new floating entry with a server-generated ID, its own single slot, and the given bounds or the default bounds `x=0.25, y=0.25, w=0.5, h=0.5`, all relative to the screen viewport. On `move_float` the server SHALL replace a floating entry's bounds. Floating and moving a floating entry SHALL NOT affect the pane's session.

#### Scenario: Float one of two panes
- **WHEN** `float_pane` is applied without bounds to one pane of a two-slot split
- **THEN** the root layout holds only the other pane's slot
- **AND** the screen gains a floating entry holding the floated pane with bounds `0.25, 0.25, 0.5, 0.5`

#### Scenario: Float the last root pane
- **WHEN** `float_pane` is applied to the only pane of the root layout
- **THEN** the op is rejected with "cannot float the last pane in the grid"

#### Scenario: Move an unknown floating entry
- **WHEN** `move_float` names a floating ID that does not exist
- **THEN** the op is rejected

### Requirement: Docking Floating Panes
On `dock_pane` the server SHALL require an existing floating entry, SHALL remove it, and SHALL append all of its panes to the root-layout slot that holds the pane named in the op, or to the first slot of the root layout when that pane is not found there. The first docked pane SHALL become the slot's active pane.

#### Scenario: Dock next to a pane
- **WHEN** a floating entry holding `p4` is docked with `paneId` naming `p1`
- **THEN** the floating entry is gone and `p1`'s slot holds `p4` as its active pane

#### Scenario: Dock an unknown floating entry
- **WHEN** `dock_pane` names a floating ID that does not exist
- **THEN** the op is rejected with "floating entry not found"

### Requirement: Active Pane of a Stack
When the active pane of a slot is removed from that slot, the server SHALL make the first remaining pane of the slot its active pane. Operations that place a pane into an existing stack (`move_pane` with `targetSlotId`, `dock_pane`) SHALL make the placed pane active as described in their requirements.

#### Scenario: Close the active tab
- **WHEN** a slot holds `[p1, p2, p3]` with `p2` active and `p2` is closed
- **THEN** the slot holds `[p1, p3]` with `p1` active

### Requirement: Workspace Outlives Clients
Screens, panes and sessions SHALL belong to the server's workspace, not to individual clients. A client disconnecting SHALL NOT remove or change any screen, pane or session, and every connected client SHALL observe the same workspace. Whether the workspace survives a server restart is defined by `server-modes`.

#### Scenario: Client disconnects
- **WHEN** the only connected client disconnects
- **THEN** all screens, panes and sessions remain in the workspace and their sessions keep running

#### Scenario: Second client sees the same screens
- **WHEN** a second client connects while a first client has created screens A and B
- **THEN** the second client is offered screens A and B in the workspace's screen order

### Requirement: Spatial Neighbor Resolution
The server SHALL resolve the neighbor of a pane in a direction (`left`, `right`, `above`/`up`, `below`/`down`) by projecting the screen's root layout onto relative rectangles using split axes and sizes, then choosing, among panes in other slots whose center lies in the 90-degree cone around the direction (the movement along the direction axis is positive and at least as large as the perpendicular offset), the one whose center is nearest to the source pane's center. Panes in the same slot and panes in floating entries SHALL NOT be neighbors. When no pane qualifies, or the source pane is not in the root layout, the result SHALL be "no neighbor".

#### Scenario: Neighbor to the right
- **WHEN** the root is a `horizontal` split of `left` and `right` and the neighbor of `left` in direction `right` is requested
- **THEN** the result is pane `right`

#### Scenario: No neighbor at the edge
- **WHEN** the neighbor of `left` in direction `left` is requested in the same layout
- **THEN** the result is "no neighbor"

#### Scenario: Stacked panes are not neighbors
- **WHEN** a slot holds `[p1, p2]` and is the only slot of the screen
- **THEN** `p1` has no neighbor in any direction

### Requirement: Managed Job Placement
`create_job` SHALL create a new job pane/session in one atomic layout operation, as a new screen, a tab in an existing root-layout slot, or a directional split of a root-layout pane. Tab/split placement SHALL require a valid screen/pane pair. The op SHALL carry explicit launch data, never inherit a source pane's command, privileges or profile, and use provided new screen/pane IDs or generate them when absent.

#### Scenario: Direct tab creation
- **WHEN** a job is placed as a tab beside `p1` in a slot containing `[p1,p2]`
- **THEN** that slot gains exactly one job pane without an intermediate split or a second process

#### Scenario: Directional job splits
- **WHEN** a job requests left, right, above or below an existing root-layout pane
- **THEN** its new slot is placed on the requested side using the corresponding horizontal or vertical axis and valid sizes

#### Scenario: Unsupported floating anchor or duplicate new ID
- **WHEN** placement names a floating pane or a new pane ID already used anywhere in the workspace
- **THEN** creation is rejected atomically without any process or job allocation

### Requirement: Background Job Activation
Creating a job SHALL preserve existing active screen and active tabs. A new screen SHALL become active only if none existed. A tab in a previously empty slot SHALL become that slot's active pane. Placement SHALL not authorize a global focus or clipboard command to clients.

#### Scenario: Background job beside current work
- **WHEN** a job is added to an active tab stack or as a split while another pane holds keyboard focus
- **THEN** the active screen and existing active tab remain unchanged and no focus command is emitted

### Requirement: Job Lifecycle State Changes
Job starts, failures, exits, stop requests, ownership changes and forget operations SHALL update authoritative job/session state through validated operations. Spawn failure SHALL leave an inspectable failed job, not a running-session claim. Closing a job pane or its screen SHALL initiate its managed stop and retain terminal metadata; pending starts SHALL not publish a late live pane after that close.

#### Scenario: Pane closed while spawn is pending
- **WHEN** a user closes a job pane while its process start is pending
- **THEN** the accepted attempt is cancelled or any process it started is stopped, and a late completion never resurrects that pane

#### Scenario: Job pane moved
- **WHEN** a user moves a running job pane to another slot
- **THEN** the job keeps its session, ownership and cursor generation

### Requirement: Moving a Pane to Another Screen
On `move_pane_to_screen` with the source screen as the envelope's `screenId`, a `paneId` and a `targetScreenId`, the server SHALL move the pane and its session record from the source to the target without starting, ending or signalling the session. An existing target SHALL take the pane as the active tab of the first slot of its grid, or as its only pane when the grid is empty.

#### Scenario: Move to an existing screen
- **WHEN** screen `s1` holds panes `p1` and `p2` and `move_pane_to_screen` moves `p2` to screen `s2`
- **THEN** `p2` and its session are in `s2` as the active tab of its first slot and no longer in `s1`
- **AND** no session is started or ended

### Requirement: New Screen from a Pane Move
With `createScreen: true`, `move_pane_to_screen` SHALL create the target as a new screen with that ID, named `screenName` when given, holding only the pane and placed right after the source in the screen order.

#### Scenario: Move to a new screen
- **WHEN** `move_pane_to_screen` moves `p2` from `s1` to `s9` with `createScreen` true and `screenName` "Logs"
- **THEN** a screen `s9` named "Logs" holds only `p2` and follows `s1` in the screen order

### Requirement: Rejected Pane Moves
The server SHALL reject `move_pane_to_screen`, leaving the workspace and every revision unchanged, when the source or an existing target is unknown, a created target already exists, the target is the source, the source does not hold the pane, or the pane belongs to a managed job.

#### Scenario: Rejected move
- **WHEN** `move_pane_to_screen` names a pane its source screen does not hold
- **THEN** the op is rejected and the workspace, including both revisions, is unchanged

#### Scenario: Job pane stays
- **WHEN** `move_pane_to_screen` names a managed job's pane
- **THEN** the op is rejected and the pane stays in its screen

### Requirement: Source Screen Emptied by a Move
A screen that `move_pane_to_screen` leaves without panes SHALL be removed with the same op, as `close_screen` removes it, ending no session; when it was the active screen, the target SHALL become active.

#### Scenario: Last pane moved away
- **WHEN** screen `s1` holds only pane `p1` and `p1` moves to `s2`
- **THEN** `s1` no longer exists, `p1`'s session keeps running in `s2`, and no session is ended

### Requirement: Arranging the Panes of a Screen
On `arrange_panes` the server SHALL replace the screen's root layout with the tree in `root`, made of `split` nodes (an `axis`, two or more `children` and one size per child) and `slot` nodes (`paneIds` in tab order and an optional `activePaneId`), and SHALL assign new node IDs to it. Pane IDs the screen does not hold SHALL be left out, together with a slot or split they leave empty or with a single child. The server SHALL then require that the tree holds every pane of the root layout exactly once and no other pane of the screen, that every slot holds a pane, that every split has at least two children and sizes as `resize_split` requires them, and that an `activePaneId` is one of its slot's panes, rejecting the op otherwise. Only a screen whose root layout holds no panes SHALL accept a root slot without panes. The op SHALL create, end or change no pane, session or floating entry, and SHALL produce no effects.

#### Scenario: Tabs and splits rearranged
- **WHEN** the root is a `vertical` split `[slot(p1), horizontal split [slot(p2), slot(p3)]]` and `arrange_panes` sends a `vertical` split `[slot(p1, p3) with p3 active, slot(p2)]` with sizes `[0.7, 0.3]`
- **THEN** the root layout is that tree, with new node IDs
- **AND** the panes and their sessions are unchanged, and no session is spawned or ended

#### Scenario: A pane is missing
- **WHEN** `arrange_panes` leaves out pane `p3` of the root layout
- **THEN** the op is rejected and the layout is unchanged

#### Scenario: A pane twice
- **WHEN** `arrange_panes` lists pane `p1` in two slots
- **THEN** the op is rejected and the layout is unchanged

#### Scenario: A floating pane in the grid
- **WHEN** pane `p4` is floating and `arrange_panes` lists it in a slot
- **THEN** the op is rejected and `p4` stays floating

#### Scenario: A pane the screen does not hold
- **WHEN** `arrange_panes` lists every pane of the root layout and in addition, in a slot of its own, a pane ID the screen does not hold
- **THEN** the op is applied without that pane and without its slot

#### Scenario: Sizes that do not fit
- **WHEN** `arrange_panes` sends a split with three children and two sizes
- **THEN** the op is rejected and the layout is unchanged

### Requirement: Selecting a Tab
On `select_tab` the server SHALL require that `paneId` exists in a slot of the screen's root layout or of a floating entry, rejecting the op otherwise, and SHALL make it that slot's active pane. The op SHALL change no tab order, split, size, node ID, floating entry, pane or session, and SHALL produce no effects.

#### Scenario: Another tab of a stack
- **WHEN** a slot holds `[p1, p2, p3]` with `p3` active and `select_tab` names `p1`
- **THEN** the slot holds `[p1, p2, p3]` with `p1` active, under the same node ID
- **AND** no session is spawned or ended

#### Scenario: A tab of a floating group
- **WHEN** a floating entry holds `[p4, p5]` with `p5` active and `select_tab` names `p4`
- **THEN** the floating entry holds `p4` as its active pane, with the same bounds

#### Scenario: A pane in no slot
- **WHEN** `select_tab` names a pane ID the screen does not hold
- **THEN** the op is rejected and the layout is unchanged
