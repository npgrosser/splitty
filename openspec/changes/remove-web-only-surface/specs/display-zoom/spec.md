## MODIFIED Requirements

### Requirement: Zoom Settings
The level SHALL be the setting `appearance.zoom` (default 1.0) and its scope `appearance.zoom_scope` (`window`, the default, or `terminals`), stored with the other settings of the server that serves the window. The server SHALL accept an update of the level only to one of the levels its metadata lists, and SHALL refuse any other with status 400 and the field `appearance.zoom`; a stored level outside the listed range falls back to the default. A window SHALL offer the listed levels as its zoom setting's choices, and SHALL show any level within them, without a range of its own. With `window`, the desktop app SHALL scale its whole window, with layout and text scaled together. With `terminals`, chrome SHALL stay at 100 % and each terminal's font size SHALL be its configured or native default size times the level, rounded to whole pixels. OS-owned dialogs follow platform scaling.

#### Scenario: Level survives a restart
- **WHEN** the user zooms to 125 % in the desktop app and restarts it
- **THEN** the window opens at 125 %

#### Scenario: Only the terminals
- **WHEN** `appearance.zoom_scope` is `terminals`, the configured terminal font size is 14 and the user zooms to 150 %
- **THEN** the terminals use font size 21 and the drawer and tabs keep their size

#### Scenario: Rounded terminal font size
- **WHEN** `appearance.zoom_scope` is `terminals`, a terminal's font size is 14 and the level is 110 %
- **THEN** the terminal uses font size 15

#### Scenario: Level outside the list
- **WHEN** a client sends `appearance.zoom` 1.3 to a server that lists the levels from 50 to 300 %
- **THEN** the server answers 400 with the field `appearance.zoom` and the stored level stays

#### Scenario: Native window scope
- **WHEN** a native workspace loads or changes zoom with scope `window`
- **THEN** its terminal content and application controls scale together, with pointer targets matching their visible positions
- **AND** display DPI and persisted pane layout ratios are unchanged
