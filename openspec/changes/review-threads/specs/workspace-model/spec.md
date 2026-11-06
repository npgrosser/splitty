## MODIFIED Requirements

### Requirement: Layout Operations
The server SHALL change the workspace only through operations: workspace-level `create_screen`, `close_screen`, `rename_screen`, `set_active_screen`, `reorder_screens`, `move_pane_to_screen`, and screen-level `split_pane`, `new_tab`, `close_pane`, `resize_split`, `move_pane`, `arrange_panes`, `select_tab`, `float_pane`, `dock_pane`, `move_float`, `set_scratchpad`, `open_viewer`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies`, `mark_sent`, plus `create_job`, `stop_job`, `forget_job`, trusted job lifecycle/ownership operations, and trusted viewer operations the server derives from control API requests: an agent's reply to an annotation, and the start and end of a wait for a review (see `viewer-annotations`). An operation SHALL have the same effect on the model whether it arrives over `mux-protocol` or `control-api`, except `set_scratchpad`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` and `mark_sent`, which only `mux-protocol` accepts (see `session-scratchpad` and `viewer-annotations`). Job operations SHALL preserve the authorization and idempotency rules of `managed-jobs` at every entry point; lifecycle and ownership updates SHALL not be accepted as arbitrary client-authored state, and neither SHALL the trusted viewer operations over any transport.

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
