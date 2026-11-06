## MODIFIED Requirements

### Requirement: Reopening A File
When `open_viewer` without `newPaneId` carries a `file` source whose path a viewer pane of the screen already shows, the server SHALL add no pane. It SHALL take the first such pane in layout order (the root layout depth first, then the floating entries), make it its slot's active pane, increase its `revision` by one, and, when the op's `paneId` yields an origin as for a new viewer pane, set its `originPaneId` to that origin. When `open_viewer` without `newPaneId` carries a `text` source with `replace` set, and a viewer pane of the screen shows a text document with the same title whose origin is the origin the op yields, the server SHALL take the first such pane in layout order, replace its text with the op's, make it its slot's active pane and increase its `revision` by one; the pane's annotations stay. With `newPaneId`, the server SHALL add a new pane in any case.

#### Scenario: An agent opens its plan again
- **WHEN** a viewer pane `v1` of screen `s1` shows `/home/me/app/PLAN.md` with `revision` 2 in a slot whose active pane is another tab, and `open_viewer` without `newPaneId` names the same path on `s1`
- **THEN** `s1` holds no new pane, `v1` is its slot's active pane, and its `revision` is 3

#### Scenario: Another terminal opens the plan
- **WHEN** viewer pane `v1` was opened from terminal pane `p1` and `splitty open` of the same file runs in terminal pane `p2` of the same screen
- **THEN** `v1` is shown again with `originPaneId` `p2`

#### Scenario: A window opens the same file
- **WHEN** a viewer pane shows `/home/me/app/PLAN.md` and `open_viewer` names the same path with `newPaneId` `viewer-0123456789abcdef`
- **THEN** the screen holds a second viewer pane `viewer-0123456789abcdef` for that file

#### Scenario: A revised plan
- **WHEN** viewer pane `v1` shows a text titled `Plan` opened from terminal pane `p1` and holds annotations, and `open_viewer` without `newPaneId` from `p1` carries a new text titled `Plan` with `replace` set
- **THEN** the screen holds no new pane, `v1` shows the new text with its `revision` raised by one, and it keeps its annotations

#### Scenario: Another title
- **WHEN** the same op carries the title `Notes` instead
- **THEN** a new viewer pane shows the text, and `v1` stays as it was
