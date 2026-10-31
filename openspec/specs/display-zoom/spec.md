# display-zoom Specification

## Purpose
TBD - created by archiving change zoom. Update Purpose after archive.

## Requirements

### Requirement: Zoom Shortcuts
The client SHALL zoom in with Cmd (macOS) or Ctrl (Linux, Windows) and `+` or `=`, zoom out with `-`, and return to 100 % with `0`, also from a focused terminal, which SHALL NOT receive these keys. The level SHALL move in the steps 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250 and 300 % and stay within them, and the client SHALL briefly show the new level. In a browser, with the whole window as the zoom's scope, the client SHALL leave these keys to the browser.

#### Scenario: Zoom in from a terminal
- **WHEN** a terminal has the focus at 100 % and the user presses Cmd+`+` on macOS
- **THEN** the level becomes 110 % and the terminal receives no input

#### Scenario: Bounded steps
- **WHEN** the level is 300 % and the user zooms in
- **THEN** the level stays 300 %

### Requirement: Zoom Settings
The level SHALL be the setting `appearance.zoom` (0.5 to 3.0, default 1.0) and its scope `appearance.zoom_scope` (`window`, the default, or `terminals`), stored with the other settings of the server that serves the window; an invalid value falls back to the default. With `window`, a desktop shell SHALL zoom its whole page through its native page zoom. With `terminals`, the page SHALL stay at 100 % and each terminal's font size SHALL be the configured one times the level.

#### Scenario: Level survives a restart
- **WHEN** the user zooms to 125 % in the desktop app and restarts it
- **THEN** the window opens at 125 %

#### Scenario: Only the terminals
- **WHEN** `appearance.zoom_scope` is `terminals`, the configured terminal font size is 14 and the user zooms to 150 %
- **THEN** the terminals use font size 21 and the drawer and tabs keep their size
