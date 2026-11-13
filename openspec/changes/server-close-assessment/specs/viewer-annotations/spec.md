# Spec Delta

## MODIFIED Requirements

### Requirement: Confirmation Before Annotations Are Lost
Closing a viewer pane that holds annotations SHALL ask first, as closing a pane whose scratchpad holds text does; closing several panes at once SHALL ask once for all. Quitting a window whose embedded server would end viewer panes that hold annotations SHALL ask first as well. Quitting a window of a daemon SHALL NOT ask for annotations, since its panes continue. Every window SHALL learn how many annotations a close ends from the server's close assessment (see `mux-protocol`, Close Assessment).

#### Scenario: Close a viewer with annotations
- **WHEN** the user closes a viewer pane that holds three annotations
- **THEN** a confirmation names the annotations, and the pane closes only when the user confirms

#### Scenario: Quit an embedded window
- **WHEN** the user quits a window of an embedded server while a viewer pane holds annotations
- **THEN** the window asks before quitting

#### Scenario: Close the native window
- **WHEN** the user closes the native window of its owned local workspace while a viewer pane on a hidden Screen holds annotations
- **THEN** the confirmation counts those annotations
