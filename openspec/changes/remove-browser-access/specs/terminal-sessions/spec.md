## MODIFIED Requirements

### Requirement: Client Handling of Session Streams
The server streams a session at most once per connection, so a client SHALL keep at most one `attach` for a session in flight on a connection, and SHALL send `detach` for a session only when none of its views shows that session or waits for it. Binary output frames for a session that arrive after the client sent `attach` and before the matching `attached` event are that session's buffered output; the client SHALL display them, in order, before any output that follows. When a view is replaced by another view for the same session while its attach is in flight, the new view SHALL take over that attach and receive its buffered output and all later output. Historical output SHALL NOT perform clipboard operations or produce replies to terminal report requests, the status probe `OSC 7501 ; ?` among them. Live output after `attached` SHALL retain its normal behavior even when buffered before a view is wired. The server SHALL filter clipboard operations and terminal report requests from replay before sending it over any transport. Clients SHALL retain their replay protection for older servers.

#### Scenario: Buffered output shown on attach
- **WHEN** a client attaches a view to a session whose buffered output ends with a shell prompt
- **THEN** the view shows that prompt before any output produced after the attach

#### Scenario: View replaced while its attach is in flight
- **WHEN** a view's attach is in flight and a new view for the same session replaces it
- **THEN** the client sends that session one `attach` and no `detach`
- **AND** the new view shows the buffered output and keeps receiving new output

#### Scenario: Last view closed
- **WHEN** the only view of a session is closed without closing the session
- **THEN** the client sends `detach` for that session

#### Scenario: Resume with old queries and copies
- **WHEN** historical output includes terminal color/status queries or OSC 52 clipboard operations, including across frame boundaries
- **THEN** restoring the view neither sends their replies into the session nor changes the clipboard
- **AND** live queries and clipboard writes after `attached` behave normally

#### Scenario: Status probe in a replay from an older server
- **WHEN** the replay of an older server, which forwards status probes, holds `ESC ] 7501 ; ? BEL`
- **THEN** `splitty attach` does not pass the probe on to a terminal that could answer it

### Requirement: Client Size Reports
With a server that announces `pty-size-v1`, a client SHALL report as `resize` the size of a shown pane (its panel visible on the active screen), report it again when the pane becomes shown, and send `hide` when it stops being shown. It SHALL report no other pane, except that it MAY report the other terminal tabs of a shown pane's tab group at that pane's measured size, follow that size, and send `hide` for them once the group stops being shown; the native app does. Its first report after an attach SHALL carry `redraw: true` instead of resizing the PTY by one row and back. With other servers it SHALL resize the PTY as before, send none of these messages and resize no background tab.

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
