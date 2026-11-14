## MODIFIED Requirements

### Requirement: Layout Operations
The server SHALL change the workspace only through operations: workspace-level `apply_template`, `create_screen`, `close_screen`, `rename_screen`, `set_active_screen`, `reorder_screens`, `move_pane_to_screen`, `move_tab_group_to_screen`, and screen-level `split_pane`, `new_tab`, `close_pane`, `resize_split`, `move_pane`, `move_tab`, `arrange_panes`, `select_tab`, `float_pane`, `dock_pane`, `move_float`, `set_scratchpad`, `open_viewer`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies`, `mark_sent`, plus `create_job`, `stop_job`, `forget_job`, trusted job lifecycle/ownership operations, and trusted viewer operations the server derives from control API requests: an agent's reply to an annotation, and the start and end of a wait for a review (see `viewer-annotations`). An operation SHALL have the same effect on the model whether it arrives over `mux-protocol` or `control-api`, except `apply_template`, `set_scratchpad`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` and `mark_sent`, which only `mux-protocol` accepts (see `templates`, `session-scratchpad` and `viewer-annotations`). Job operations SHALL preserve the authorization and idempotency rules of `managed-jobs` at every entry point; lifecycle and ownership updates SHALL not be accepted as arbitrary client-authored state, and neither SHALL the trusted viewer operations over any transport.

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

#### Scenario: Annotations only through the window
- **WHEN** a `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` or `mark_sent` op arrives over the control API
- **THEN** the server refuses it, and the workspace stays unchanged

#### Scenario: No client writes an agent's reply
- **WHEN** a client submits an operation that would add an agent's reply or start a wait for a review, over the WebSocket or the control API
- **THEN** the server refuses it, and the workspace stays unchanged

## ADDED Requirements

### Requirement: Moving a Tab Group to Another Screen
On `move_tab_group_to_screen` with the source screen as the envelope's `screenId`, a `slotId` naming a slot of its root layout or of a floating entry, and a `targetScreenId`, the server SHALL move every pane of that slot and its session record to the target in one operation, without starting, ending or signalling a session. The panes SHALL join the first slot of the target's grid in their tab order, or be its only panes when the grid is empty, and `activePaneId`, else the slot's active pane, else its first, SHALL be that slot's active tab. With `createScreen: true` the target SHALL be created as for `move_pane_to_screen`, named `screenName` or by the server, right after the source. A source left without panes SHALL be removed with the op, ending no session, and when it was active the target SHALL become active.

#### Scenario: Group to an existing screen
- **WHEN** screen `s1` holds `p1` beside the group `[p3, p4]` and `move_tab_group_to_screen` moves that group to `s2`, whose first slot holds `p2`, with `activePaneId` `p3`
- **THEN** `s2`'s first slot holds `[p2, p3, p4]` with `p3` active, `s1` holds only `p1`, and both screens' revisions rose by one
- **AND** no session is started or ended

#### Scenario: Group to a new screen
- **WHEN** the group `[p3, p4]` of `s1` moves to `s9` with `createScreen` true and `screenName` "Logs"
- **THEN** a screen `s9` named "Logs" holds `[p3, p4]` with their sessions and follows `s1` in the screen order

#### Scenario: Last group moved away
- **WHEN** the active screen `s1` holds only the group `[p1, p3]` and it moves to `s2`
- **THEN** `s1` no longer exists, `s2` is active, and both sessions keep running in `s2`

#### Scenario: Floating group
- **WHEN** a floating entry's group `[p3, p5]` moves to `s2`
- **THEN** the entry is gone from the source and `p3` and `p5` are in `s2`'s grid, not floating

### Requirement: Rejected Tab Group Moves
The server SHALL reject `move_tab_group_to_screen`, leaving the workspace and every revision unchanged, when the source or an existing target is unknown, a created target already exists, the target is the source, `slotId` names no node or a split or an empty slot, `activePaneId` is not one of the slot's panes, or any of the slot's panes belongs to a managed job.

#### Scenario: Group with a job's pane
- **WHEN** the group `[p3, p4]` holds the managed job pane `p4` and is moved to `s2`
- **THEN** the op is rejected and both panes stay in their screen

#### Scenario: Unknown group
- **WHEN** `move_tab_group_to_screen` names a slot its source screen does not hold
- **THEN** the op is rejected and the workspace, including both revisions, is unchanged

### Requirement: Native Tab Group Transfer
The native tab menu SHALL offer moving the whole tab group of the tab it was opened on to another Screen of the same server, or to a new Screen, when that group holds more than one tab, tiled or floating. The window SHALL capture the group's slot, its tabs with their sessions in order and the tab it shows in the group, SHALL send one `move_tab_group_to_screen` naming that tab as `activePaneId` only while the slot still holds exactly those tabs with those sessions and none of them belongs to a managed job, and SHALL verify and follow the move as a single tab's transfer: every moved pane on the target with its session before it shows the target with that tab.

#### Scenario: Group moved from a tab's menu
- **WHEN** the menu of tab `p1` in the group `[p0, p1, p2]` showing `p0` moves the group to Screen `other`
- **THEN** the window sends one `move_tab_group_to_screen` with the group's slot, `activePaneId` `p0` and `targetScreenId` `other`
- **AND** once the move is verified it shows `other` with `p0`

#### Scenario: Group changed after the menu opened
- **WHEN** a tab joined or left the group, or a session changed, before the move is chosen
- **THEN** the window sends nothing and says that the group changed

#### Scenario: Single tab
- **WHEN** the tab's group holds only that tab
- **THEN** the menu offers moving the tab, not the group
