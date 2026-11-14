# Spec Delta

## MODIFIED Requirements

### Requirement: Layout Operations
The server SHALL change the workspace only through operations: workspace-level `apply_template`, `create_screen`, `close_screen`, `rename_screen`, `set_active_screen`, `reorder_screens`, `move_screen`, `move_pane_to_screen`, `move_tab_group_to_screen`, and screen-level `split_pane`, `new_tab`, `close_pane`, `resize_split`, `move_pane`, `move_tab`, `arrange_panes`, `select_tab`, `float_pane`, `dock_pane`, `move_float`, `set_scratchpad`, `open_viewer`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies`, `mark_sent`, plus `create_job`, `stop_job`, `forget_job`, trusted job lifecycle/ownership operations, and trusted viewer operations the server derives from control API requests: an agent's reply to an annotation, and the start and end of a wait for a review (see `viewer-annotations`). An operation SHALL have the same effect on the model whether it arrives over `mux-protocol` or `control-api`, except `apply_template`, `set_scratchpad`, `put_annotations`, `remove_annotations`, `put_replies`, `remove_replies` and `mark_sent`, which only `mux-protocol` accepts (see `templates`, `session-scratchpad` and `viewer-annotations`). Job operations SHALL preserve the authorization and idempotency rules of `managed-jobs` at every entry point; lifecycle and ownership updates SHALL not be accepted as arbitrary client-authored state, and neither SHALL the trusted viewer operations over any transport.

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

#### Scenario: Relative Screen placement through either interface
- **WHEN** an identical valid `move_screen` is applied through the session protocol or the control API against identical workspaces
- **THEN** both produce the same Screen order without changing Screen content or sessions

### Requirement: Screen Rename, Activation and Order
The server SHALL apply `rename_screen` and `set_active_screen` only to existing screens and SHALL reject them otherwise. On `reorder_screens` the server SHALL set the screen order to the given list of screen IDs, ignoring IDs that do not name an existing screen.

On `move_screen`, the server SHALL place the named existing ordered Screen immediately before an existing ordered anchor, or at the current end when no anchor is given, using the payload in `docs/protocol.ts`. It SHALL apply the intent to current state, retaining every other order entry and all Screens omitted from the order as omitted. Repeated entries for the moved Screen SHALL become one entry; other duplicates SHALL remain unchanged. Missing or unordered moved/anchor Screens SHALL reject the operation atomically. A valid self-anchor SHALL be a no-op. No accepted placement SHALL change active Screen identity, Screen revisions, layouts, panes or sessions, or produce process effects. Legacy `reorder_screens` semantics SHALL remain unchanged.

#### Scenario: Activate an unknown screen
- **WHEN** `set_active_screen` names a screen that does not exist
- **THEN** the op is rejected and the active screen is unchanged

#### Scenario: Reorder with a stale ID
- **WHEN** `reorder_screens` is applied with `[C, X, A, B]` where X does not exist
- **THEN** the screen order becomes `[C, A, B]`

#### Scenario: Relative placement preserves a concurrent Screen
- **WHEN** a client observed `[A, B, C]`, another client appended D, and the first client then moves C before A
- **THEN** the order becomes `[C, A, B, D]`, and the active Screen and all Screen content and sessions remain unchanged

#### Scenario: Relative end uses current state
- **WHEN** another client appended D before `move_screen` moves A to the end of `[A, B, C, D]`
- **THEN** the order becomes `[B, C, D, A]` without replacing the other client's addition

#### Scenario: Missing or unlisted placement identity
- **WHEN** the moved Screen or its anchor was deleted or omitted from the current order before `move_screen` applies
- **THEN** the operation is rejected with no workspace change, even if an omitted Screen still exists

#### Scenario: Partial and duplicate order remains bounded to the moved Screen
- **WHEN** X exists but is unlisted, the order is `[B, B, A, A, C]`, and A is moved before B
- **THEN** the order becomes `[A, B, B, C]`, X remains unlisted, and all Screen records remain unchanged

#### Scenario: Placement does not navigate
- **WHEN** an already correctly placed Screen is moved to the same position or names itself as the anchor
- **THEN** the effective order and active Screen stay unchanged and no session effect occurs

## ADDED Requirements

### Requirement: Native Local Screen Placement
A native local Screen placement SHALL capture its connection, Screen and optional same-source anchor, submit `move_screen` at most once while those identities remain current, and use authoritative catalog order. It SHALL retain selected Screen identity, terminal state and sessions. Cancellation or rejected admission SHALL send no placement. Failed or uncertain submission SHALL NOT be replayed.

#### Scenario: Native order follows the service
- **WHEN** a valid native local placement is admitted
- **THEN** its new order is shown from the authoritative catalog ordered after that operation, and selection and existing terminal bindings stay unchanged
- **AND** an earlier in-flight catalog cannot be mistaken for that operation's result

#### Scenario: Stale or cancelled native placement
- **WHEN** the local connection changed, a captured Screen or anchor disappeared, the destination belongs to another source, or the gesture was cancelled before admission
- **THEN** the client submits no placement and does not retarget the intent to the current selection or a replacement source

#### Scenario: Failed or uncertain native placement
- **WHEN** the service rejects a placement or its result cannot be observed after connection loss
- **THEN** the native client retains or refreshes authoritative order without recreating Screens, claiming an unobserved success, or replaying the placement
