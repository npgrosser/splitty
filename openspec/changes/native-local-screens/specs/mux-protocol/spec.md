## ADDED Requirements

### Requirement: Ordered Workspace Catalog
A service announcing `workspace-catalog-v1` SHALL answer `mux.workspace_resync` on the existing session stream with `mux.workspace_snapshot`. Its ordered `screens` and nullable `activeScreenId` hint SHALL describe one coherent catalog, without pane layouts. Duplicate IDs SHALL appear once at their first position; an active ID absent from the catalog SHALL be returned as null. The request SHALL be processed after preceding operations on that connection. Payloads are defined in `docs/protocol.ts`.

#### Scenario: Catalog after lifecycle operations
- **WHEN** a client sends create, rename or close operations followed by `mux.workspace_resync`
- **THEN** the response reflects those completed operations, including the removal of the last screen
- **AND** the same stream continues delivering terminal output in order

#### Scenario: Coherent order and active screen
- **WHEN** a workspace catalog is requested during concurrent workspace edits
- **THEN** the returned screen names, order and active-screen identifier belong to one coherent state
- **AND** duplicate order entries appear once and an active identifier absent from the returned catalog becomes null

#### Scenario: Additive capability
- **WHEN** the welcome does not announce `workspace-catalog-v1`
- **THEN** a client SHALL NOT send `mux.workspace_resync` or infer support from successful transport negotiation
- **AND** existing per-screen resync behavior remains unchanged
