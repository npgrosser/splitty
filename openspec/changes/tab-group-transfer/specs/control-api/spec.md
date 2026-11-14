## MODIFIED Requirements

### Requirement: Operation Targeting
The server SHALL apply the op types `create_screen`, `close_screen`, `rename_screen`, `set_active_screen`, `move_pane_to_screen` and `move_tab_group_to_screen` as workspace operations and the existing other layout op types as screen operations on the screen named by `screenId`. `close_screen`, `rename_screen` and `set_active_screen` target `screenId`; `move_pane_to_screen` and `move_tab_group_to_screen` move from `screenId`; `create_screen` ignores the envelope's `screenId`, takes the new screen's id from the op's own `screenId` field or generates one, and uses the default shell profile when the op names none. A screen operation whose `paneId` the named screen does not hold, while another screen does, SHALL be applied to the screen that holds the pane, so a shell whose pane moved keeps targeting it; a managed job's pane never moves, so for it the mismatch stays a conflicting target. Managed-job operations SHALL target their declared placement or job ID, as specified in `workspace-model`, and reject conflicting envelope targets rather than retargeting silently; an owner's tab or split placement whose screen no longer holds its pane SHALL follow the pane, an agent grant's SHALL NOT (see `access-control`).

#### Scenario: Screen op routed to the request's screen
- **WHEN** a client posts `{"screenId": "s2", "op": {"type": "close_pane", "paneId": "p7"}}`
- **THEN** the server closes pane `p7` in screen `s2`

#### Scenario: New screen with a client-chosen id
- **WHEN** a client posts `{"screenId": "", "op": {"type": "create_screen", "screenId": "screen-9"}}`
- **THEN** the workspace contains a new screen with id `screen-9` whose first session uses the default shell profile

#### Scenario: Split in a moved shell
- **WHEN** pane `p7` moved from screen `s1` to `s2` and its shell runs `splitty split`, which posts a `split_pane` of `p7` on `s1`
- **THEN** the server splits `p7` in `s2`

#### Scenario: Tab group moved through the control API
- **WHEN** a client posts `{"screenId": "s1", "op": {"type": "move_tab_group_to_screen", "slotId": "n3", "targetScreenId": "s2"}}` for the slot `n3` of `s1` holding `p3` and `p4`
- **THEN** `p3` and `p4` with their sessions are in `s2`'s first slot, in that order

#### Scenario: Tab group move without its target
- **WHEN** a client posts a `move_tab_group_to_screen` without `slotId` or `targetScreenId`
- **THEN** the server responds with status 400 and an error starting with `invalid op:`, and the workspace is unchanged

### Requirement: Operation Effects and Client Notification
An operation applied through `POST /api/mux/op` SHALL take full effect, including starting and terminating the terminal sessions it implies (see `terminal-sessions`). After an accepted op every client connection, native or through the browser gateway, SHALL receive one `mux.pane_spawned` per successfully spawned pane and then one `mux.nudge` with reason `cli_op` and the actual affected screen ID, in that order (see `mux-protocol`); after an accepted `move_pane_to_screen` or `move_tab_group_to_screen`, one such nudge for the source and one for the target. A failed spawn SHALL emit no successful spawn event for that session; any retained failed job state SHALL still be synchronized. State validation rejection SHALL notify nobody.

#### Scenario: Split from the CLI reaches the UI
- **WHEN** a CLI splits a pane through `POST /api/mux/op` while a UI client is connected
- **THEN** a session is started for the new pane
- **AND** the UI client receives `mux.pane_spawned` for the new pane and then `mux.nudge` with reason `cli_op`

#### Scenario: Op without spawned sessions
- **WHEN** a CLI renames a screen through `POST /api/mux/op`
- **THEN** every client connection receives a `mux.nudge` with reason `cli_op` and no `mux.pane_spawned`

#### Scenario: Move from the CLI
- **WHEN** a pane moves from `s1` to `s2` through `POST /api/mux/op` while a UI client is connected
- **THEN** the UI client receives `mux.nudge` with reason `cli_op` for `s1` and for `s2`
