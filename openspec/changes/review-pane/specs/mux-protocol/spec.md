## MODIFIED Requirements

### Requirement: Change Propagation to Other Clients
When an op submitted over the WebSocket spawns or terminates sessions, the server SHALL send every other connected client the op's `mux.pane_spawned` events followed by `mux.nudge` with reason `mux_op` and the op's `screenId`; the originating client SHALL NOT receive that nudge. When a `move_pane_to_screen` op is applied, the server SHALL send every other connected client `mux.nudge` with reason `mux_op` for the source screen and for the target screen, and none to the originating client. When a `set_scratchpad`, an `arrange_panes`, an `open_viewer`, a `put_annotations` or a `remove_annotations` op is applied, or a `close_pane` op that ends no session, the server SHALL send every other connected client `mux.nudge` with reason `mux_op` for its screen, and none to the originating client.

#### Scenario: Session-changing op from a client
- **WHEN** client A sends a `mux.op` on screen `s1` that spawns a session while client B is connected
- **THEN** client B SHALL receive `mux.pane_spawned` and then `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive the nudge

#### Scenario: Pane moved by a client
- **WHEN** client A moves a pane from screen `s1` to screen `s2` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` for `s1` and for `s2`
- **AND** client A SHALL receive neither

#### Scenario: Scratchpad saved by a client
- **WHEN** client A sends `set_scratchpad` on screen `s1` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive it

#### Scenario: Panes arranged by a client
- **WHEN** client A sends an accepted `arrange_panes` on screen `s1` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive it

#### Scenario: Viewer opened and closed by a client
- **WHEN** client A opens a viewer pane on screen `s1` with `open_viewer` and later closes it with `close_pane` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1` after each of the two ops
- **AND** client A SHALL receive neither

#### Scenario: Annotation added by a client
- **WHEN** client A sends `put_annotations` on screen `s1` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` and `screenId` `s1`
- **AND** client A SHALL NOT receive it

### Requirement: Rejected Op Reporting
When the server rejects a recognized op that names a pane to create (`newPaneId`) because it fails validation (see `workspace-model`), it SHALL send `mux.op_failed` to the originating client only, carrying the op `type`, the `screenId`, that `paneId`, and an error message. When it rejects a recognized `arrange_panes`, `put_annotations` or `remove_annotations` op because it fails validation, it SHALL send `mux.op_failed` to the originating client only, carrying the op `type`, the `screenId` and an error message, and for the annotation ops the viewer pane's `paneId`.

#### Scenario: Rejected pane-creating op
- **WHEN** a client sends a `mux.op` with `newPaneId` `p9` that the server rejects
- **THEN** the client SHALL receive `mux.op_failed` with `paneId` `p9` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected arrangement
- **WHEN** a client sends `arrange_panes` on screen `s1` that misses one of its panes
- **THEN** the client SHALL receive `mux.op_failed` with `op` `arrange_panes`, `screenId` `s1` and a non-empty `error`
- **AND** no other client SHALL receive it

#### Scenario: Rejected annotation
- **WHEN** a client sends `put_annotations` for viewer pane `v1` on screen `s1` that would give the pane a 101st annotation
- **THEN** the client SHALL receive `mux.op_failed` with `op` `put_annotations`, `screenId` `s1`, `paneId` `v1` and a non-empty `error`
- **AND** no other client SHALL receive it
