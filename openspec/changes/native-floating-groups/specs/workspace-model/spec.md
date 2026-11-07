## MODIFIED Requirements

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

#### Scenario: Close within a nested floating entry
- **WHEN** the last tab in one slot of a floating split is closed
- **THEN** only that pane's session ends, its empty slot is removed and surviving sibling slots and sessions remain in the floating entry
- **AND** a parent left with one child collapses without removing the remaining floating content

### Requirement: Floating Panes
On `float_pane` the server SHALL require a pane in the root layout and SHALL reject the op when that pane is the last pane of the root layout. The pane SHALL be removed from the root layout (a parent split's sizes become equal fractions and a single-child split collapses) and placed in a new floating entry with a server-generated ID, its own single slot, and the given bounds or the default bounds `x=0.25, y=0.25, w=0.5, h=0.5`, all relative to the screen viewport. On `move_float` the server SHALL replace a floating entry's bounds. Both operations SHALL reject any non-finite bounds component or non-positive width or height before changing the workspace. Finite offscreen coordinates and positive dimensions larger than the viewport SHALL remain valid. Floating and moving a floating entry SHALL NOT affect the pane's session.

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

#### Scenario: Invalid float bounds
- **WHEN** `float_pane` or `move_float` supplies any non-finite bounds component or a zero or negative width or height
- **THEN** the operation is rejected and layout, sessions and effects remain unchanged

#### Scenario: Valid offscreen or oversized bounds
- **WHEN** `float_pane` or `move_float` supplies finite negative coordinates or positive dimensions greater than one
- **THEN** the supplied bounds remain authoritative without a forced viewport clamp

### Requirement: Native Screen Transfer Admission
The native client SHALL bind a Screen-transfer action to its captured connection, source Screen, terminal pane and session. It SHALL validate that source and the same-service destination before submitting `move_pane_to_screen` once. A menu for a non-selected tab SHALL target that tab. Stale, managed-job, conflicting or unsupported actions SHALL submit no transfer. Opening or dismissing the menu SHALL submit no operation.

#### Scenario: Transfer a non-selected tab
- **WHEN** a transfer is chosen for a non-selected terminal tab whose captured source still exists in the shown Screen
- **THEN** the submitted operation names that tab and its chosen destination, without first selecting a different source

#### Scenario: Stale or conflicting action
- **WHEN** the captured connection, Screen, pane or session changed, the pane is managed by a job, the destination is invalid, or an incompatible operation is pending
- **THEN** the client rejects the action without submitting a transfer or retargeting it to the focused tab

#### Scenario: Transfer a floating tab
- **WHEN** a captured floating terminal tab is moved to an existing or new Screen
- **THEN** the same source identity checks apply and the unchanged pane/session pair moves to the target's tiled root

## ADDED Requirements

### Requirement: Native Floating Layout Projection
The native client SHALL display canonical floating groups alongside the tiled layout and retain their pane/session identity through float, dock and geometry changes. Selection, new tabs, close, search and Screen transfer SHALL use their existing semantics on floating tabs. Operations defined only for the tiled root SHALL remain unavailable on floating surfaces. External floating changes SHALL use the same projection.

#### Scenario: Observe and manipulate floating terminals
- **WHEN** a terminal is floated locally or by another client, receives another tab, moves, resizes or docks
- **THEN** its authoritative group is displayed and its session, retained terminal content and supported tab actions continue without detach or replay

#### Scenario: Dock a floating group
- **WHEN** the native Dock action is confirmed for a floating group
- **THEN** all of its panes move to the first tiled slot without restarting their sessions, including when the tiled root is empty

### Requirement: Native Floating Focus and Visibility
The native client's focus, tab cycling and visible-size reporting SHALL include floating groups. Maximizing any canonical slot SHALL show only that slot without changing the server layout; restoring SHALL reveal the other groups. Hidden groups SHALL retain terminal output without claiming a visible PTY size. Local stacking changes SHALL NOT mutate the workspace.

#### Scenario: Floating tabs and maximization
- **WHEN** a floating tab is selected or cycled, its slot is maximized, and the view is later restored or its Screen is revisited
- **THEN** focus and selection stay consistent with the displayed canonical slots and each displayed terminal reports its measured size
- **AND** hidden terminals continue processing output without visible-size claims

### Requirement: Native Floating Gesture Admission
The native client SHALL bind float, dock and bounds gestures to their captured owner and layout identities. Move/resize release SHALL submit at most one changed operation. Stale or cancelled gestures SHALL submit none. Metadata-only updates SHALL preserve a valid gesture; incompatible layout, viewport or zoom changes SHALL cancel it. Outcomes SHALL be verified against authoritative state without replaying a mutation.

#### Scenario: Valid gesture and metadata update
- **WHEN** a floating group is moved or resized while only its process metadata changes
- **THEN** release submits one changed bounds operation and the authoritative result retains the same terminal sessions

#### Scenario: Cancelled or stale gesture
- **WHEN** Escape, window deactivation, owner/Screen/viewport/zoom change or an incompatible authoritative layout update invalidates a floating gesture
- **THEN** the gesture submits no operation and does not overwrite the later state

#### Scenario: Uncertain floating result
- **WHEN** a floating action is rejected or cannot be verified after submission
- **THEN** the client reports its identified outcome, keeps the authoritative projection and does not automatically submit the mutation again

### Requirement: Native Floating Pointer Isolation
Native floating surfaces SHALL route pointer and wheel input to the topmost hit surface without also delivering it to terminals behind that surface. Header and resize gestures SHALL NOT become terminal input. Reachability adjustments for offscreen or oversized stored bounds SHALL remain local presentation changes until an explicit layout gesture is submitted.

#### Scenario: Overlapping groups and reachable bounds
- **WHEN** floating groups overlap tiled or floating terminals, including after viewport resizing with offscreen or oversized stored bounds
- **THEN** only the topmost hit terminal receives application pointer input, controls remain reachable, and viewport adjustment alone sends no bounds operation
