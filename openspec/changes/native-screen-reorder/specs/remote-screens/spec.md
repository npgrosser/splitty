# Spec Delta

## MODIFIED Requirements

### Requirement: Remote Screens In The Window
A client SHALL show local screens and remote screens grouped by their host in the screen list. Local screens SHALL come first; the host groups SHALL follow in the order the user arranged them (see `remote-hosts`, Host Group Order), and hosts the user did not arrange SHALL follow the arranged ones in the order their screens first appear. The user SHALL be able to move a host group, with its screens, above or below another host group by dragging its caption; the local screens SHALL NOT be movable as a group, and a screen SHALL NOT move to another host's group. It SHALL hold one hub connection per host with open screens or an explicit request to browse that host, and SHALL speak the mux and session protocols over it with that host's daemon. A remote screen SHALL be a reference to a screen of a host. Its layout and sessions are the host's, and every op on it SHALL go to that host. Local settings (theme, font, keybindings) SHALL apply to remote screens. A new tab or split in a remote screen SHALL send no local shell profile. A remote screen's group SHALL identify the host and its connection state. Screen switching and shortcut numbering SHALL follow the grouped display order.

A native window SHALL allow rearranging its opened remote Screen references within their existing host group, including offline or missing references. This SHALL change only that window's reference order, retain selected identity and terminal state, and preserve the arrangement through live catalog refresh and host reconnect. It SHALL NOT send a remote workspace-order operation, connect a host, open or recreate a Screen, or change another group's order. A cancelled action or an action naming a removed reference, different source, or obsolete local window owner SHALL leave reference order unchanged.

#### Scenario: New screen on a host
- **WHEN** the user creates a screen on `vm1`
- **THEN** it appears under `vm1`, and shows connection progress until its first pane runs on `vm1`

#### Scenario: Split in a remote pane
- **WHEN** the user runs `splitty split` in a pane of a remote screen
- **THEN** the pane is split on the host, and the local workspace is unchanged

#### Scenario: Grouped navigation
- **WHEN** local screens and screens from multiple hosts are open
- **THEN** screen navigation and shortcut numbers follow the same host-group order shown in the drawer

#### Scenario: Host group moved
- **WHEN** the screen list shows local screens, then `vm1`, then `vm2`, and the user drags the caption of `vm2` above `vm1`
- **THEN** the list shows local screens, then `vm2` with its screens, then `vm1` with its screens
- **AND** the shortcut numbers of the screens follow that order
- **AND** the order is the same after the client restarts

#### Scenario: Native reference order is window-local
- **WHEN** a native window moves an opened remote Screen before another reference of the same host
- **THEN** only that group's reference order changes, navigation follows that order, and the selected Screen and sessions stay unchanged
- **AND** no remote workspace-order operation is sent and other windows and host groups retain their order

#### Scenario: Native offline ordering survives reconnect
- **WHEN** a native window rearranges offline or missing references and the host later supplies a catalog or reconnects
- **THEN** the references retain their window order and normal missing-Screen behavior, without the reorder action connecting, opening or recreating anything

#### Scenario: Native reference placement becomes stale
- **WHEN** a reference placement is cancelled or its local window owner, source or required reference is no longer current
- **THEN** the window changes no reference order and sends no host operation
