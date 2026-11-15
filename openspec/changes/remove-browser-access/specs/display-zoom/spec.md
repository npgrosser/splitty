## MODIFIED Requirements

### Requirement: Zoom Shortcuts
The client SHALL zoom in with Cmd (macOS) or Ctrl (Linux, Windows) and `+` or `=`, zoom out with `-`, and return to 100 % with `0`, also from a focused terminal, which SHALL NOT receive these keys. The level SHALL move through the levels the `values` of `appearance.zoom` in the server's settings metadata list, to the next listed level above or below the current one, and stay within them; a level between two listed ones moves to its neighbour. The server SHALL list 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250 and 300 %. A native window whose local service lists no levels, with or without metadata, SHALL move through these levels within that service's bounds. The client SHALL briefly show the new level.

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
