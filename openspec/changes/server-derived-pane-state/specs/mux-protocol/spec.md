## ADDED Requirements

### Requirement: Server Derived Pane State
The server SHALL advertise serverDerivedPaneState. Session snapshots SHALL carry displayTitle and busy, and ordinary terminal pane titles SHALL match their session display title. Metadata SHALL refresh without attached clients. Clients SHALL use these values and leave automatic exit closure to the server; older servers retain legacy client behavior.

#### Scenario: Reconnect to an unattended pane
- **WHEN** a process changes while no window shows its pane and a client later resynchronizes
- **THEN** the snapshot gives its current displayTitle and busy state and the pane title matches

#### Scenario: Older host
- **WHEN** a host does not advertise serverDerivedPaneState
- **THEN** the client retains its legacy title, launch and close-on-exit handling
