## ADDED Requirements

### Requirement: Native Local Screen Workflow

The experimental native desktop SHALL render its local screen from the assigned service's authoritative screen snapshot. Creating a split or tab, closing a pane and resizing a split SHALL use existing mux operations and preserve the service's ownership of terminal sessions. Selecting a tab SHALL change the window-local presentation state and, when the service announces `tab-select-v1`, SHALL persist the choice through `select_tab` as `mux-protocol` requires, without changing another window's shown tab. Unchanged panes SHALL retain their terminal view and session attachment across layout updates. Input and output SHALL remain scoped to the pane's assigned session and current connection generation. The local development workspace SHALL retain the isolated application lifetime rather than modifying an unrelated service's workspace.

#### Scenario: Split and add a tab
- **WHEN** a user splits a terminal and adds a tab in one of the resulting groups
- **THEN** the native window renders the service's resulting layout and each terminal pane streams its distinct service-created session

#### Scenario: Select and resize
- **WHEN** a user switches a tab or drags a split divider
- **THEN** the window shows the selected tab without changing another window's selection, and a divider resize is confirmed by the service's screen snapshot
- **AND** unchanged terminal panes retain their existing view and session state

#### Scenario: Close a local pane
- **WHEN** a user closes a terminal pane in the local screen
- **THEN** the service applies the existing close semantics and the window removes only views absent from the resulting authoritative snapshot
- **AND** output or queued input for the closed pane cannot be routed to another pane

#### Scenario: New terminal after closing the final pane
- **WHEN** the local screen has no panes and the user creates a terminal
- **THEN** the service recreates the screen with its existing ID and name and owns the new terminal session
