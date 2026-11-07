## MODIFIED Requirements

### Requirement: Native Local Screen Workflow

The experimental native desktop SHALL render each local screen from the assigned service's authoritative screen snapshot. Creating a split or tab, closing a pane and resizing a split SHALL use existing mux operations and preserve the service's ownership of terminal sessions. Selecting a tab SHALL change the window-local presentation state and, when the service announces `tab-select-v1`, SHALL persist the choice through `select_tab` as `mux-protocol` requires, without changing another window's shown tab. Unchanged panes SHALL retain their terminal view and session attachment across layout updates. Input and output SHALL remain scoped to the pane's assigned session and current connection generation. The local development workspace SHALL retain the isolated application lifetime rather than modifying an unrelated service's workspace.

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

## ADDED Requirements

### Requirement: Native Local Screen Catalog and Selection
The experimental native desktop SHALL create, switch, rename and close local screens through its assigned service on one connection. Screen membership, names and order SHALL follow the authoritative catalog. The service's active-screen hint SHALL initialize local choice only; later hints SHALL NOT change a valid local choice. Closing the last screen SHALL leave an empty workspace from which a new screen can be created.

#### Scenario: Lifecycle and local choice
- **WHEN** the window creates and renames a screen, switches between existing screens and closes one
- **THEN** names, membership and ordering follow the confirmed service state
- **AND** surviving screens retain their local tab and focus choices
- **AND** another client's activation does not switch this window

#### Scenario: Close the final screen
- **WHEN** the final local screen is closed
- **THEN** the service ends its sessions and the native workspace shows no screen
- **AND** creating a new screen uses new screen and pane identities

#### Scenario: Delayed catalog during creation
- **WHEN** a response to a catalog request sent before a local create arrives after that create was submitted
- **THEN** it does not cancel the new screen's pending pane or discard its arriving output
- **AND** a subsequent ordered refresh confirms the operation

#### Scenario: Unsupported workspace catalog
- **WHEN** the assigned service lacks `workspace-catalog-v1`
- **THEN** the native window reports that multi-screen control is unavailable rather than fabricating an authoritative catalog

### Requirement: Native Screen Switching Preserves Sessions
Switching the shown local screen SHALL preserve surviving terminal views, attachments and emulation state across all screens. Inactive screens SHALL continue receiving their output and sending live terminal replies, while withdrawing visibility and size claims. A returning screen SHALL publish sizes for its shown panes. User actions and replies SHALL remain scoped to their original screen, pane, session and connection generation.

#### Scenario: Hidden output and return
- **WHEN** a screen becomes inactive while its terminal produces output and requests a terminal reply
- **THEN** its terminal state advances and the reply reaches its own session without a hidden resize
- **AND** switching back shows that state with the same session and terminal view

#### Scenario: Stale callback after switching or removal
- **WHEN** an obsolete visibility, resize or input callback arrives after its screen was switched or removed
- **THEN** it cannot resize, focus or send input to a different session
- **AND** a removed screen cannot be resurrected by a delayed snapshot or spawn

### Requirement: Native Workspace Admission Across Screens
The native connection's attachment and pending-pane admission bound SHALL cover all local screens together. A pending pane SHALL reserve capacity before its creation is submitted, and a failed submission or rejected creation SHALL release it. Closing a screen SHALL release its routes and attachments only through authoritative reconciliation, and queued actions SHALL NOT redirect to another screen.

#### Scenario: Global capacity and recovery
- **WHEN** attached sessions and pending panes across several screens exhaust the connection's admission bound
- **THEN** another pane or screen creation is rejected visibly before submission
- **AND** a rejected creation or confirmed closure restores usable capacity without detaching surviving screens
