## ADDED Requirements

### Requirement: Native Tab Group Move
In a native workspace window, dragging the empty area of a tiled tab group's tab bar SHALL drag the whole group; a press there without a drag SHALL do nothing. The group SHALL have a single tab's drop targets: beside another group or the whole layout, or into another group before a tab or last. The window SHALL send the result as one `arrange_panes` of the root layout it shows, keeping each group's tab order and shown tab, and nothing for a drop on the group itself or one that changes nothing.

#### Scenario: Group dropped into another group
- **WHEN** the group `[p1, p2]` showing `p2` is dropped into the group `[p3]`
- **THEN** the window sends one `arrange_panes` whose grid holds `[p3, p1, p2]` in one slot with `p2` active, and shows `p2` there
- **AND** no pane or session is created or ended

#### Scenario: Group dropped beside another group
- **WHEN** a group is dropped on the right edge zone of another group
- **THEN** the arrangement puts it right of that group in a split of the two, and the other groups keep their places, tabs and shown tabs

#### Scenario: Drop on itself
- **WHEN** a group is dropped on its own body or tab bar
- **THEN** nothing is sent

### Requirement: Native Tab Group Move Admission
A native group drag SHALL capture its connection, Screen, root layout and the identities of the group's tabs, and the window SHALL send its `arrange_panes` only while all of them are current. Floating groups SHALL NOT be dragged as groups, and the tab lock of the settings SHALL lock group drags too. On `mux.op_failed` for that `arrange_panes` the window SHALL show the service's layout again and SHALL NOT repeat the move.

#### Scenario: Layout changed during the drag
- **WHEN** the root layout or a tab of the dragged group changes before the drop
- **THEN** the drop sends nothing

#### Scenario: Rejected group move
- **WHEN** the server answers the group's `arrange_panes` with `mux.op_failed`
- **THEN** the window shows the service's layout and the rejection as its status, and sends nothing more for the move
