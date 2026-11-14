## MODIFIED Requirements

### Requirement: Client Size Reports
With a server that announces `pty-size-v1`, a client SHALL report as `resize` the size of a shown pane (its panel visible on the active screen), report it again when the pane becomes shown, and send `hide` when it stops being shown. It SHALL report no other pane, except that it MAY report the other terminal tabs of a shown pane's tab group at that pane's measured size, follow that size, and send `hide` for them once the group stops being shown; the native app does, the web client does not. Its first report after an attach SHALL carry `redraw: true` instead of resizing the PTY by one row and back. With other servers it SHALL resize the PTY as before, send none of these messages and resize no background tab.

#### Scenario: Background screen
- **WHEN** the user switches from a host's screen to another screen of the same host
- **THEN** the client sends `hide` for the sessions of the screen it left and `resize` for those of the screen it shows

#### Scenario: Background tab
- **WHEN** the native app shows a pane at 120×40 whose tab group holds a second terminal tab, and the window grows the pane to 140×45
- **THEN** it sends `resize` with 120×40 and then 140×45 for both sessions and no `hide` for the background tab
- **AND** when the user switches to that tab, it sends no `hide` for either session, and when another Screen is shown, it sends `hide` for both

#### Scenario: Older server
- **WHEN** a client attaches to a session of a server whose `mux.welcome` has no `pty-size-v1`
- **THEN** it resizes the PTY by one row and back after the attach and sends no `activity`, `idle` or `hide`
