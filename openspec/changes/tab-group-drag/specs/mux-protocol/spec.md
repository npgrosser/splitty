## MODIFIED Requirements

### Requirement: Window Layout Changes Reach the Server
The server SHALL announce `layout-arrange-v1` in `mux.welcome.capabilities`. When the user moves a tab or a tab group by drag and drop, or drags a divider, on a screen of a server that announced it, a browser workspace window SHALL send `arrange_panes` with the arrangement of that screen's grid (its rows, columns and tab groups with their panes, tab order, active tabs and sizes; floating groups are not part of it) once the gesture is done, unless the arrangement equals the last one it received in a snapshot or sent for that screen. It SHALL send none while it applies a snapshot to that screen, while a group of the screen is maximized and while the screen has a group in a popout window, and none to a server that did not announce the capability. A tab selected without a drag SHALL NOT send `arrange_panes`; `select_tab` reports it (see Selected Tabs Reach the Server). On `mux.op_failed` for an `arrange_panes` op, the window SHALL send `mux.resync` for its screen. A browser workspace window SHALL NOT send `move_pane` for a drag. Native workspace windows use precise pane-move and split-resize operations instead of reporting complete arrangements (see `workspace-model`), except for the move of a whole tab group, which they send as one `arrange_panes` (see `workspace-model`, Native Tab Group Move).

#### Scenario: Capability announced
- **WHEN** a client connects
- **THEN** `mux.welcome.capabilities` contains `layout-arrange-v1`

#### Scenario: Tab dragged into another group
- **WHEN** the user drags, in a browser workspace window, the tab of pane `p2` into the tab group of `p1` on a screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends `arrange_panes` whose grid holds `p1` and `p2` in one slot
- **AND** after a reconnect the screen shows `p2` in that tab group

#### Scenario: Host without the capability
- **WHEN** the user drags a tab in a browser workspace window on a screen of a host whose `mux.welcome` has no `layout-arrange-v1`
- **THEN** the window sends neither `arrange_panes` nor `move_pane`

#### Scenario: Rejected arrangement
- **WHEN** the browser workspace window receives `mux.op_failed` for an `arrange_panes` it sent for screen `s1`
- **THEN** it sends `mux.resync` for `s1` and shows the snapshot's layout

#### Scenario: Native group move
- **WHEN** the user drops a whole tab group in a native workspace window on a screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends one `arrange_panes` for that screen and no `move_pane` or `move_tab`

## ADDED Requirements

### Requirement: Native Actions That Need A Capability
A native workspace window SHALL offer an action whose operation needs a `mux.welcome` capability whatever its Screen's server announced. When the user takes it on a Screen whose server's last `mux.welcome` did not list that capability, the window SHALL send nothing for it and SHALL show one status message saying that the server, for a host's Screen the host's daemon, is older than the app and needs an update. Only the announced capabilities decide, never a version.

#### Scenario: A host's daemon without the capability
- **WHEN** the user drops a tab group on a host's Screen whose daemon's `mux.welcome` has no `layout-arrange-v1`
- **THEN** no operation reaches the host, the layout stays as it was, and the window's status says the host's daemon needs an update

#### Scenario: A server that announced it
- **WHEN** the same drop is made on a Screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends its `arrange_panes`
