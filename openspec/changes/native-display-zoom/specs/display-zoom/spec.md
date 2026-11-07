## MODIFIED Requirements

### Requirement: Zoom Settings
The level SHALL be the setting `appearance.zoom` (0.5 to 3.0, default 1.0) and its scope `appearance.zoom_scope` (`window`, the default, or `terminals`), stored with the other settings of the server that serves the window; an invalid value falls back to the default. With `window`, a desktop shell SHALL scale its whole application surface, using native page zoom for web-based shells and matching layout/text scaling for the native workspace. With `terminals`, chrome SHALL stay at 100 % and each terminal's font size SHALL be its configured or native default size times the level. OS-owned dialogs follow platform scaling.

#### Scenario: Level survives a restart
- **WHEN** the user zooms to 125 % in the desktop app and restarts it
- **THEN** the window opens at 125 %

#### Scenario: Only the terminals
- **WHEN** `appearance.zoom_scope` is `terminals`, the configured terminal font size is 14 and the user zooms to 150 %
- **THEN** the terminals use font size 21 and the drawer and tabs keep their size

#### Scenario: Native window scope
- **WHEN** a native workspace loads or changes zoom with scope `window`
- **THEN** its terminal content and application controls scale together, with pointer targets matching their visible positions
- **AND** display DPI and persisted pane layout ratios are unchanged

## ADDED Requirements

### Requirement: Ordered Native Zoom Updates
Native zoom actions SHALL target only their captured local service through bounded ordered settings work, preserving terminal event delivery. Each admitted action SHALL derive its next level from current service settings and patch only zoom. Outcomes SHALL remain observable independently of appearance refreshes. Failed or uncertain writes SHALL be reported without automatic mutation retry, and a current read SHALL precede later actions.

#### Scenario: Rapid actions while settings refresh
- **WHEN** several zoom actions arrive while local settings or OS preference refreshes are in progress
- **THEN** admitted actions execute in order without being replaced or losing their outcomes to read refreshes
- **AND** terminal output and ordinary input continue, unrelated settings are preserved and queue overflow is visible

#### Scenario: Save response is uncertain
- **WHEN** a zoom save may have been applied but its outcome cannot be confirmed
- **THEN** the client reports uncertainty and reconciles with current service settings before later actions
- **AND** it does not resend the uncertain action automatically or claim that it was definitely unsaved

#### Scenario: Wrong owner or close during update
- **WHEN** a zoom action names an obsolete connection or the owning workspace closes during settings work
- **THEN** no action is redirected to another service, and owned pending API work is cancelled and joined during shutdown

### Requirement: Coherent Native Terminal Zoom
Native terminal zoom SHALL preserve session identity, retained output and search queries while using consistent geometry for painting, selection, links, mouse input and IME bounds. Applying zoom SHALL invalidate gestures based on older geometry. Hidden panes SHALL retain their desired scale and publish new dimensions only after they become shown and measured. Cell pixel geometry SHALL update even when rounded row and column counts stay equal.

#### Scenario: Existing and hidden panes
- **WHEN** zoom changes while terminal panes are visible or hidden, and a hidden pane is later selected
- **THEN** each shown pane uses the current zoom without replacing its process or losing retained output or query text
- **AND** the hidden pane makes no visible size claim before selection

#### Scenario: Changed geometry during interaction
- **WHEN** zoom changes during a terminal or pane-layout pointer gesture
- **THEN** the old gesture cannot act on the new geometry
- **AND** subsequent selection, links, mouse coordinates and input-method bounds agree with the scaled display

#### Scenario: Same grid at a different cell size
- **WHEN** zoom or display scale changes cell pixels while the rounded terminal columns and rows remain equal
- **THEN** terminal pixel geometry is refreshed without unnecessary duplicate PTY dimension reports
