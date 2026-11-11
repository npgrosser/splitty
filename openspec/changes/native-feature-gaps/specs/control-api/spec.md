## ADDED Requirements

### Requirement: Native Editor Requests
The native window SHALL register for editor requests on every connection that announced `editor-open-v1` and show a request as a notice with the editor, the host and the path. Only the user's Open SHALL launch the editor. The window SHALL answer `dispatched`, `denied`, `expired`, `failed` or `disconnected`, show at most one request per connection, and choose the editor as "Open In Client Editor" says, from the request and the `editors` settings.

#### Scenario: Declined
- **WHEN** a pane runs `splitty open --editor PLAN.md` and the user declines the notice
- **THEN** nothing is launched and the command ends with the refusal

### Requirement: Native Forwarded Commands
The native window SHALL carry out a `command` event of a service on a pane of the Screen it shows from that service: the pane the command names, or the focused one. `pane.activate` SHALL select the pane, `pane.clear` clear its terminal's buffer, `pane.close` close it as the window's own close does, with its confirmation, and `pane.split` split a tiled pane `right` or `below`. `pane.copy` and `pane.paste` SHALL act only for the local service; a host's command SHALL NOT read or write this computer's clipboard (provisional, tbd). A command for another Screen, an unknown pane or an unknown type SHALL change nothing.

#### Scenario: Split of a named pane
- **WHEN** the local service forwards `{"type":"pane.split","paneId":"p1","direction":"below"}` and the shown Screen has the pane `p1`
- **THEN** the window splits `p1` downwards

#### Scenario: Pane of another Screen
- **WHEN** a forwarded command names a pane that is not on the shown Screen of its service
- **THEN** the window does nothing
