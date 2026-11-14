## ADDED Requirements

### Requirement: Tab Group Transfer Capability
A server that takes `move_tab_group_to_screen` (see `workspace-model`, Moving a Tab Group to Another Screen) SHALL announce `tab-group-transfer-v1` in `mux.welcome.capabilities`. A client SHALL NOT send the op to a server whose welcome lacks it: such a server ignores the op without an answer. An accepted move SHALL notify every other client with `mux.nudge` with reason `mux_op` for the source and for the target, and not the sender; a parsed move that is rejected SHALL answer only its sender with `mux.op_failed` naming `move_tab_group_to_screen` and the source Screen. A `move_tab_group_to_screen` without `slotId` or `targetScreenId` is malformed and SHALL be ignored without a response.

#### Scenario: Tab group moved by a client
- **WHEN** client A moves the tab group of `p2` and `p5` from screen `s1` to screen `s2` while client B is connected
- **THEN** client B SHALL receive `mux.nudge` with reason `mux_op` for `s1` and for `s2`
- **AND** client A SHALL receive neither

#### Scenario: Rejected tab group move
- **WHEN** client A sends `move_tab_group_to_screen` on `s1` naming a slot `s1` does not hold
- **THEN** client A SHALL receive `mux.op_failed` with `op` `move_tab_group_to_screen`, `screenId` `s1` and a non-empty `error`
- **AND** no other client SHALL receive it, and the workspace is unchanged

#### Scenario: Older server
- **WHEN** a client's welcome lacks `tab-group-transfer-v1`
- **THEN** the client sends no `move_tab_group_to_screen` to that server

## MODIFIED Requirements

### Requirement: Native Actions That Need A Capability
A native workspace window SHALL offer an action whose operation needs a `mux.welcome` capability whatever its Screen's server announced. When the user takes it on a Screen whose server's last `mux.welcome` did not list that capability, the window SHALL send nothing for it, SHALL settle any pending state of the action as rejected, and SHALL show one status message saying that the server, for a host's Screen the host's daemon, is older than the app and needs an update. Only the announced capabilities decide, never a version. A refusal that comes back from a server that did take the operation SHALL be shown as an ordinary status, and the window SHALL stay usable.

#### Scenario: A host's daemon without the capability
- **WHEN** the user drops a tab group on a host's Screen whose daemon's `mux.welcome` has no `layout-arrange-v1`
- **THEN** no operation reaches the host, the layout stays as it was, and the window's status says the host's daemon needs an update

#### Scenario: A server that announced it
- **WHEN** the same drop is made on a Screen of a server that announced `layout-arrange-v1`
- **THEN** the window sends its `arrange_panes`

#### Scenario: A group transfer to an older server
- **WHEN** the user moves a tab group to another Screen of a local service or a host whose `mux.welcome` has no `tab-group-transfer-v1`
- **THEN** no `move_tab_group_to_screen` is sent, the group stays where it was, and the window's status says the service, or the host's daemon, needs an update

#### Scenario: A group transfer the server refuses
- **WHEN** the server answers the window's `move_tab_group_to_screen` with `mux.op_failed`
- **THEN** the window shows the rejection as its status, keeps the group where the server has it, and the next move can be made
