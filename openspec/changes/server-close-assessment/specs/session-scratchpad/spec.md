# Spec Delta

## MODIFIED Requirements

### Requirement: Confirmation Before Text Is Lost
Closing a pane whose scratchpad holds text SHALL ask first, as closing a pane with a running program does; closing several panes at once SHALL ask once for all. Quitting a window whose embedded server would end sessions whose scratchpads hold text SHALL ask first as well. Quitting a window of a daemon SHALL NOT ask for scratchpads, since its sessions continue. Every window SHALL learn which scratchpads hold text from the server's close assessment (see `mux-protocol`, Close Assessment), in the web and the native client alike.

#### Scenario: Close a pane with text
- **WHEN** the user closes an idle shell pane whose scratchpad holds text
- **THEN** a confirmation names the scratchpad, and the pane closes only when the user confirms

#### Scenario: Close a native pane with text
- **WHEN** the user closes an idle shell pane with scratchpad text in the native window
- **THEN** the native window asks before it closes the pane

#### Scenario: Quit an embedded window
- **WHEN** the user quits a window of an embedded server while a session's scratchpad holds text
- **THEN** the window asks before quitting
