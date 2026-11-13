## MODIFIED Requirements

### Requirement: Workspace Restore On Daemon Start
When a daemon starts and a readable `state.json` exists, it SHALL restore that workspace, keeping the screen, pane, and session identifiers. If the file is missing or cannot be parsed, the daemon SHALL start with an empty workspace. A pane of the legacy role `webview` SHALL be removed as if it had been closed, keeping the rest of its Screen's layout; a Screen left without panes SHALL be removed.

#### Scenario: Restart restores layout
- **WHEN** a daemon is stopped and started again
- **THEN** clients see the same screens, names, order, and layouts with the same identifiers as before the stop

#### Scenario: Corrupt state file
- **WHEN** `state.json` contains invalid JSON at daemon start
- **THEN** the daemon starts with an empty workspace

#### Scenario: Legacy web pane
- **WHEN** `state.json` holds a Screen with a terminal pane and a `webview` pane split side by side
- **THEN** the daemon restores the Screen with the terminal pane alone in its layout, with its session and identifiers

#### Scenario: Older host's web pane in a window
- **WHEN** a host of an earlier release sends a Screen holding a pane of the role `webview`
- **THEN** the window reads that pane as a terminal pane and keeps the Screen's layout as the host sent it
