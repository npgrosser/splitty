## ADDED Requirements

### Requirement: Native Pane Move Admission
A native workspace pane-move gesture SHALL capture its originating connection, layout structure, pane and session. It SHALL submit `move_pane` only while those identities and the root-layout destination remain current. Cancellation or rejected admission SHALL submit no operation and SHALL retain the authoritative layout.

#### Scenario: Gesture becomes stale
- **WHEN** the connection, layout structure or captured pane-session route changes before a drop is admitted
- **THEN** the gesture is rejected without sending a move for the replacement state

#### Scenario: Cancelled or unavailable destination
- **WHEN** a gesture is cancelled, dropped outside a visible destination, or its destination disappears
- **THEN** it sends no layout operation and existing sessions remain attached

### Requirement: Native Pane Move Reconciliation
A native workspace SHALL retain a moved terminal's session and terminal state, preserve the relative sizes of surviving source siblings, and await authoritative placement before selecting and focusing the moved pane at its destination. A rejected move SHALL NOT create a local replacement layout or replay the move automatically.

#### Scenario: Move confirms placement
- **WHEN** a native pane move is admitted and its authoritative snapshot confirms the destination
- **THEN** the moved pane becomes selected there with its original session and terminal state
- **AND** surviving source siblings retain their relative proportions

#### Scenario: Earlier snapshot or rejected move
- **WHEN** an earlier snapshot still places the moving pane at its source, or the server rejects the move
- **THEN** the client does not select a fictitious destination or retry the move
