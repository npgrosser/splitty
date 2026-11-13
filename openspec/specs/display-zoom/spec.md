# display-zoom Specification

## Purpose
Display zoom controls how application surfaces and terminal content are enlarged, using bounded keyboard steps and persisted per-server appearance settings.

## Requirements

### Requirement: Zoom Shortcuts
The client SHALL zoom in with Cmd (macOS) or Ctrl (Linux, Windows) and `+` or `=`, zoom out with `-`, and return to 100 % with `0`, also from a focused terminal, which SHALL NOT receive these keys. The level SHALL move through the levels the `values` of `appearance.zoom` in the server's settings metadata list, to the next listed level above or below the current one, and stay within them; a level between two listed ones moves to its neighbour. The server SHALL list 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250 and 300 %. A native window whose local service lists no levels, with or without metadata, SHALL move through these levels within that service's bounds; a web window without metadata SHALL only return to 100 %. The client SHALL briefly show the new level. In a browser, with the whole window as the zoom's scope, the client SHALL leave these keys to the browser.

#### Scenario: Zoom in from a terminal
- **WHEN** a terminal has the focus at 100 % and the user presses Cmd+`+` on macOS
- **THEN** the level becomes 110 % and the terminal receives no input

#### Scenario: Bounded steps
- **WHEN** the level is 300 % and the user zooms in
- **THEN** the level stays 300 %

#### Scenario: Levels from the server
- **WHEN** a server's metadata lists the levels 25, 50, 100, 200 and 400 % and the level is 200 %
- **THEN** zooming in sets 400 %, and the window shows 400 %

#### Scenario: Level between the listed ones
- **WHEN** the stored level is 117 % and the user zooms in, or out
- **THEN** the level becomes 125 %, or 110 %

#### Scenario: Older local service
- **WHEN** a native window's local service publishes `appearance.zoom` without `values`
- **THEN** the keys step through the levels from 50 to 300 % as before

### Requirement: Zoom Settings
The level SHALL be the setting `appearance.zoom` (default 1.0) and its scope `appearance.zoom_scope` (`window`, the default, or `terminals`), stored with the other settings of the server that serves the window. The server SHALL accept an update of the level only to one of the levels its metadata lists, and SHALL refuse any other with status 400 and the field `appearance.zoom`; a stored level outside the listed range falls back to the default. A window SHALL offer the listed levels as its zoom setting's choices, and SHALL show any level within them, without a range of its own. With `window`, a desktop shell SHALL scale its whole application surface, using native page zoom for web-based shells and matching layout/text scaling for the native workspace. With `terminals`, chrome SHALL stay at 100 % and each terminal's font size SHALL be its configured or native default size times the level, rounded to whole pixels, in every client. OS-owned dialogs follow platform scaling.

#### Scenario: Level survives a restart
- **WHEN** the user zooms to 125 % in the desktop app and restarts it
- **THEN** the window opens at 125 %

#### Scenario: Only the terminals
- **WHEN** `appearance.zoom_scope` is `terminals`, the configured terminal font size is 14 and the user zooms to 150 %
- **THEN** the terminals use font size 21 and the drawer and tabs keep their size

#### Scenario: Rounded terminal font size
- **WHEN** `appearance.zoom_scope` is `terminals`, a terminal's font size is 14 and the level is 110 %
- **THEN** the terminal uses font size 15 in the web and in the native client

#### Scenario: Level outside the list
- **WHEN** a client sends `appearance.zoom` 1.3 to a server that lists the levels from 50 to 300 %
- **THEN** the server answers 400 with the field `appearance.zoom` and the stored level stays

#### Scenario: Native window scope
- **WHEN** a native workspace loads or changes zoom with scope `window`
- **THEN** its terminal content and application controls scale together, with pointer targets matching their visible positions
- **AND** display DPI and persisted pane layout ratios are unchanged

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
