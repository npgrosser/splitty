## MODIFIED Requirements

### Requirement: Status in the Window
A native window SHALL keep the root records each connection reports, scoped to that connection, and SHALL show on a pane's tab and on the pane's row in the drawer whether its program works, is blocked (with the kind), is done or failed. For an idle record both places SHALL show that the pane holds a program that reports its status, unless the pane's title starts with an icon of its own, which then stays. A screen and a host group SHALL show that a pane in them is blocked, also for a host's screens the window has not opened. A `done` or `error` mark SHALL go once the user has seen the pane and return only for a newer record; the pane then shows as an idle one does. A host that is not connected or does not announce `program-status-v1` shows no status.

#### Scenario: Blocked pane on a screen not opened
- **WHEN** a window is connected to a host, has not opened one of its screens, and a pane on that screen becomes `blocked`
- **THEN** the host group in the drawer shows that a pane is blocked, and the host's list of screens marks that screen

#### Scenario: Done seen
- **WHEN** a pane on another screen shows `done` and the user switches to that screen, where that pane has the focus
- **THEN** the mark goes and does not return when the connection is replaced and reports the same record again

#### Scenario: The same session id on two connections
- **WHEN** a host and the local server each report a record for a session with the same id
- **THEN** each record marks only the pane of its own connection

#### Scenario: Older host
- **WHEN** a host's `mux.welcome` does not list `program-status-v1`
- **THEN** its panes, screens and host group show no status

#### Scenario: Working pane in the drawer
- **WHEN** a pane holds a `working` root record
- **THEN** its tab and its row in the drawer both show that the program works

#### Scenario: Idle pane with a title icon
- **WHEN** a pane holds an `idle` root record and its title starts with an icon
- **THEN** its tab and its row in the drawer show the title's icon and no status mark
