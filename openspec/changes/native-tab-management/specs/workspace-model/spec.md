# Spec Delta

## MODIFIED Requirements

### Requirement: Layout Operations
The server SHALL change the workspace only through operations: workspace-level `apply_template`, `create_screen`, `close_screen`, `rename_screen`, `set_active_screen`, `reorder_screens`, `move_pane_to_screen`, and screen-level `split_pane`, `new_tab`, `close_pane`, `resize_split`, `move_pane`, `move_tab`, `arrange_panes`, `select_tab`, `float_pane`, `dock_pane`, `move_float`, `set_scratchpad`, `open_viewer`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies`, `mark_sent`, plus `create_job`, `stop_job`, `forget_job`, trusted job lifecycle/ownership operations, and trusted viewer operations the server derives from control API requests: an agent's reply to an annotation, and the start and end of a wait for a review (see `viewer-annotations`). An operation SHALL have the same effect on the model whether it arrives over `mux-protocol` or `control-api`, except `apply_template`, `set_scratchpad`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` and `mark_sent`, which only `mux-protocol` accepts (see `templates`, `session-scratchpad` and `viewer-annotations`). Job operations SHALL preserve the authorization and idempotency rules of `managed-jobs` at every entry point; lifecycle and ownership updates SHALL not be accepted as arbitrary client-authored state, and neither SHALL the trusted viewer operations over any transport.

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

### Requirement: Active Pane of a Stack
When the active pane of a slot is removed from that slot, the server SHALL make the first remaining pane of the slot its active pane. Operations that place a pane into an existing stack (`move_pane` with `targetSlotId`, cross-slot `move_tab`, `dock_pane`) SHALL make the placed pane active as described in their requirements.

#### Scenario: Close the active tab
- **WHEN** a slot holds `[p1, p2, p3]` with `p2` active and `p2` is closed
- **THEN** the slot holds `[p1, p3]` with `p1` active

### Requirement: Native Pane Move Admission
A native workspace pane-move gesture SHALL capture its originating connection, layout structure, pane and session. It SHALL submit `move_pane` or capability-supported `move_tab` only while those identities and the root-layout destination and any insertion anchor remain current. Cancellation or rejected admission SHALL submit no operation and SHALL retain the authoritative layout.

#### Scenario: Gesture becomes stale
- **WHEN** the connection, layout structure or captured pane-session route changes before a drop is admitted
- **THEN** the gesture is rejected without sending a move for the replacement state

#### Scenario: Cancelled or unavailable destination
- **WHEN** a gesture is cancelled, dropped outside a visible destination, or its destination disappears
- **THEN** it sends no layout operation and existing sessions remain attached

### Requirement: Native Pane Move Reconciliation
A native workspace SHALL retain a moved terminal's session and terminal state, preserve the relative sizes of surviving source siblings, and await authoritative placement before selecting and focusing a pane moved between groups. Same-group tab reorder SHALL retain selection and local maximization. Later local navigation SHALL cancel deferred focus. A rejected move SHALL NOT create a local replacement layout or replay the move automatically.

#### Scenario: Move confirms placement
- **WHEN** a native cross-group pane move is admitted and its authoritative snapshot confirms the destination
- **THEN** the moved pane becomes selected there with its original session and terminal state
- **AND** surviving source siblings retain their relative proportions

#### Scenario: Earlier snapshot or rejected move
- **WHEN** an earlier snapshot still places the moving pane at its source, or the server rejects the move
- **THEN** the client does not select a fictitious destination or retry the move

#### Scenario: Reorder without changing selection
- **WHEN** a tab is reordered within its existing group
- **THEN** its original session and terminal state, selected tab and local maximization are retained

#### Scenario: Exact placement is superseded
- **WHEN** authoritative state ordered after an admitted exact tab placement shows another position, or verification fails
- **THEN** the pending placement settles without selecting a fictitious destination or replaying the operation
- **AND** an earlier snapshot alone cannot settle the placement

#### Scenario: Navigation while placement is pending
- **WHEN** a user selects another pane or Screen before an admitted cross-group tab placement is confirmed
- **THEN** confirmation does not steal that later focus choice

## ADDED Requirements

### Requirement: Exact Tab Placement
`move_tab` SHALL move one root-layout pane from its named source slot into its named target slot, immediately before a destination pane or at the end when the anchor is omitted, using the payload in docs/protocol.ts. It SHALL preserve pane/session records and surviving layout identities. Same-slot reorder SHALL preserve the effective active pane. Cross-slot placement SHALL activate the moved pane and preserve the relative sizes of surviving source siblings.

#### Scenario: First, middle and last positions
- **WHEN** a tab is placed before the first or a middle tab, or with no anchor
- **THEN** it occupies exactly that position without changing the other tabs' relative order or creating process effects

#### Scenario: Implicit active pane survives reorder
- **WHEN** a slot has no explicit active pane and its first pane is moved within that slot
- **THEN** the same pane remains effectively active after the reorder

#### Scenario: Empty source collapses
- **WHEN** cross-slot placement removes the source slot's last pane
- **THEN** the empty source collapses, surviving siblings retain their current relative weights, and the destination activates the moved pane

### Requirement: Exact Tab Placement Validation
`move_tab` SHALL reject a pane outside its named root source slot, a missing root destination, an anchor outside the destination, or a self-anchor. Rejection SHALL leave the entire workspace and its revisions unchanged. Placement SHALL use the current authoritative layout without replacing unrelated concurrent state.

#### Scenario: Invalid source or insertion anchor
- **WHEN** the pane's source changed, the destination or anchor disappeared, the anchor moved to another slot, or the anchor names the moved pane
- **THEN** the operation is rejected atomically

#### Scenario: Unrelated concurrent edit
- **WHEN** unrelated metadata or split proportions changed while the named source, target and anchor remain valid
- **THEN** placement preserves those changes and uses the current source proportions
